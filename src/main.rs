extern crate dirs;
extern crate notify;
extern crate regex;
extern crate rusqlite;
extern crate rust_stemmers;
extern crate unicode_normalization;

use notify::DebouncedEvent::{
    Chmod, Create, Error, NoticeRemove, NoticeWrite, Remove, Rename, Rescan, Write,
};
use notify::{watcher, RecursiveMode, Watcher};
use regex::Regex;
use rusqlite::{params, params_from_iter, Connection, Statement};
use rust_stemmers::{Algorithm, Stemmer};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug)]
struct MonitoredFile {
    id: u32,
    modified: u64,
    path: String,
}

#[derive(Debug)]
struct WordStem {
    id: u32,
    stem: String,
}

fn main() {
    let punc = Regex::new(r"[\x00-\x26\x28-\x2F\x3A-\x40\x5B-\x60\x7B-\x7F]+").unwrap();
    let acc = Regex::new(r"\x{0300}-\x{035f}").unwrap();
    let stem = Stemmer::create(Algorithm::English);
    let (config_path, db_path) = find_paths();
    let config_file = fs::read_to_string(config_path.as_path())
        .expect("Unable to read configuration file.");
    let config = gjson::parse(&config_file);
    let (tx, rx) = channel();
    let check_period = config.get("period").u64();
    let mut watcher = watcher(tx, Duration::from_secs(check_period)).unwrap();
    let sqlite = Connection::open(db_path.as_path()).unwrap();
    let start = SystemTime::now();

    enforce_data_model(&sqlite);

    let mut fileq = sqlite
        .prepare("SELECT id, modified, path FROM monitored_file where path = ?")
        .unwrap();

    for folder in config.get("folder").array() {
        let recurse = folder.get("recurse").bool();
        let mode = if recurse {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        let folder_name = folder.get("name");
        let path = folder_name.str();

        process_folder(
            &sqlite, path, recurse, &punc, &acc, &stem, &mut fileq,
        );
        watcher.watch(path, mode).unwrap();
    }

    println!("Indexing complete.  Monitoring...");
    match SystemTime::now().duration_since(start) {
        Ok(n) => println!("{} seconds", n.as_secs()),
        Err(_) => panic!("Something bad"),
    }
    loop {
        match rx.recv() {
            Ok(event) => match event {
                Chmod(event) => println!("{:?}", event),
                Create(epath) => {
                    let path = epath.to_str().unwrap();
                    let last_modified = file_mod_time(path);
                    process_file(
                        &sqlite,
                        path,
                        &punc,
                        &acc,
                        &stem,
                        last_modified,
                        &mut fileq,
                    );
                }
                Error(event, _path) => println!("{:?}", event),
                NoticeRemove(event) => println!("{:?}", event),
                NoticeWrite(event) => println!("{:?}", event),
                Remove(event) => println!("{:?}", event),
                Rename(old, new) => println!("{:?} => {:?}", old, new),
                Rescan => println!("{:?}", event),
                Write(path) => println!("{:?}", path),
            },
            Err(e) => println!("watch error: {:?}", e),
        }
    }
}

// Iterate through the files in the folder, adding or indexing any files
// that are new or updated since our last run.
fn process_folder(
    sqlite: &Connection,
    path: &str,
    recursive: bool,
    punc: &Regex,
    acc: &Regex,
    stem: &Stemmer,
    fileq: &mut Statement,
) {
    let dir = Path::new(path);

    if !dir.is_dir() {
        return;
    }

    for entry in fs::read_dir(dir).expect("Cannot read directory") {
        let entry = entry.expect("No entry");
        let last_modified = file_mod_time(entry.path().to_str().unwrap());
        let entry_path = entry.path();
        let path_str = entry_path.to_str().unwrap();

        if recursive && entry.path().is_dir() {
            process_folder(sqlite, path_str, recursive, punc, acc, stem, fileq);
        } else if entry.path().is_dir() {
            // Should probably do something, but for now, it's just to prevent
            // directories from falling through to be managed as normal files.
        } else {
            process_file(
                sqlite,
                path_str,
                punc,
                acc,
                stem,
                last_modified,
                fileq,
            );
        }
    }
}

// Decide how to index a specific file.
fn process_file(
    sqlite: &Connection,
    path_str: &str,
    punc: &Regex,
    acc: &Regex,
    stem: &Stemmer,
    last_modified: u64,
    fileq: &mut Statement,
) {
    let mod_time = select_file(fileq, path_str);

    match mod_time {
        Some(some_mod) => {
            // Update and index an existing file.
            let mtime = some_mod.unwrap();
            if mtime.modified < last_modified {
                update_file_mod_time(sqlite, &last_modified, &path_str);
                index_file(
                    sqlite,
                    path_str,
                    mtime.id,
                    punc,
                    acc,
                    stem,
                    last_modified,
                    fileq,
                );
            }
        }
        None => {
            // Create and index a new file.
            let mod_time = insert_file(sqlite, fileq, &path_str, &last_modified);

            index_file(
                sqlite,
                path_str,
                mod_time.unwrap().unwrap().id,
                punc,
                acc,
                stem,
                last_modified,
                fileq,
            );
        }
    }
}

// Create the inverted index for the specified file.
fn index_file(
    sqlite: &Connection,
    path: &str,
    mut file_id: u32,
    punc: &Regex,
    accents: &Regex,
    stemmer: &Stemmer,
    last_modified: u64,
    fileq: &mut Statement,
) {
    let text = fs::read_to_string(path).unwrap();
    let alpha_only = punc.replace_all(&text, " ");
    let mut space_split = alpha_only.split_whitespace();
    let mut word_count = 0;
    let mut all_stems = select_all_stems(sqlite);
    let mut new_stems = Vec::<String>::new();

    // Delete any existing index.
    if file_id > 0 {
        clear_index_for(sqlite, file_id);
    } else {
        let mod_time = insert_file(sqlite, fileq, path, &last_modified);

        file_id = mod_time.unwrap().unwrap().id;
    }

    space_split.filter(|w| !punc.is_match(w)).for_each(|word| {
        let stem = stem_word(word, accents, stemmer);

        // Add the stem to the to-be-created list if necessary.
        if !all_stems.contains_key(&stem) {
            new_stems.push(stem);
        }
    });

    all_stems = insert_bulk_stems(sqlite, new_stems);
    space_split = alpha_only.split_whitespace();
    space_split.filter(|w| !punc.is_match(w)).for_each(|word| {
        let stem = stem_word(word, accents, stemmer);
        let stem_id = all_stems[&stem];
        insert_word_tuple(sqlite, file_id, stem_id, word_count, &word);
        word_count += 1;
    });
}

// Ensure the required tables are available.
fn enforce_data_model(sqlite: &Connection) {
    sqlite
        .execute(
            "CREATE TABLE IF NOT EXISTS monitored_file (
              id INTEGER PRIMARY KEY,
              path TEXT NOT NULL,
              modified INTEGER
            )",
            [],
        )
        .unwrap();
    sqlite
        .execute(
            "CREATE TABLE IF NOT EXISTS word_stem (
              id INTEGER PRIMARY KEY,
              stem TEXT NOT NULL
            )",
            [],
        )
        .unwrap();
    sqlite
        .execute(
            "CREATE TABLE IF NOT EXISTS file_reverse_index (
              id INTEGER PRIMARY KEY,
              file INTEGER NOT NULL,
              stem INTEGER NOT NULL,
              offset INTEGER NOT NULL,
              word TEXT NOT NULL,
              FOREIGN KEY(file) REFERENCES monitored_file(id),
              FOREIGN KEY(stem) REFERENCES word_stem(id)
            )",
            [],
        )
        .unwrap();
}

// Extract information from application configuration file at:
//   ~/.config/intern/intern.json
fn find_paths() -> (PathBuf, PathBuf) {
    let mut config_path = dirs::config_dir().expect("Can't access configuration folder.");
    config_path.push("intern");
    config_path.push("intern.json");

    let mut db_path = dirs::config_dir().expect("Can't access configuration folder.");
    db_path.push("intern");
    db_path.push("intern.sqlite3");

    (config_path, db_path)
}

// Get the modification time of a file.
fn file_mod_time(path: &str) -> u64 {
    let metadata = fs::metadata(path).unwrap();
    metadata
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// Get the stem for the current word.
fn stem_word(
    word: &str,
    accents: &Regex,
    stem: &Stemmer,
) -> String {
    let nfd = word.to_string().nfd().collect::<String>();
    let no_accents = accents.replace_all(&nfd, "").to_lowercase();
    stem.stem(&no_accents).to_string()
}

// Retrieve file information.
fn select_file(
    fileq: &mut Statement,
    path_str: &str,
) -> Option<Result<MonitoredFile, rusqlite::Error>> {
    let mod_times = fileq
        .query_map(params![path_str], |row| {
            Ok(MonitoredFile {
                id: row.get(0).unwrap(),
                modified: row.get(1).unwrap(),
                path: row.get(2).unwrap(),
            })
        })
        .unwrap();

    mod_times.last()
}

// Retrieve all stem information.
fn select_all_stems(
    sqlite: &Connection,
) -> HashMap<String, u32> {
    let mut result = HashMap::new();
    let mut stemq = sqlite.prepare("SELECT id, stem FROM word_stem").unwrap();
    let stem_iter = stemq.query_map([], |row| {
        Ok(WordStem {
            id: row.get(0).unwrap(),
            stem: row.get(1).unwrap(),
        })
    })
    .unwrap();

    for stem in stem_iter {
        let raw_stem = stem.unwrap();

        result.insert(
            raw_stem.stem.to_string(),
            raw_stem.id,
        );
    }
    result
}

// Add a file to be indexed.
fn insert_file(
    sqlite: &Connection,
    fileq: &mut Statement,
    path_str: &str,
    last_modified: &u64,
) -> Option<Result<MonitoredFile, rusqlite::Error>> {
    sqlite
        .execute(
            "INSERT
               INTO monitored_file (path, modified)
               VALUES (?, ?)
            ",
            params![path_str, last_modified],
        )
        .unwrap();
    select_file(fileq, path_str)
}

// Insert a group of stems.
fn insert_bulk_stems(
    sqlite: &Connection,
    stems: Vec<String>,
) -> HashMap<String, u32> {
  let placeholders = stems.iter().map(|_| "(?)").collect::<Vec<_>>().join(", ");
  let query = format!("INSERT INTO word_stem (stem) VALUES {}", placeholders);
  sqlite.execute(&query, params_from_iter(stems.iter())).unwrap();
  select_all_stems(sqlite)
}

// Index the file-stem-position tuple.
fn insert_word_tuple(
    sqlite: &Connection,
    file_id: u32,
    stem_id: u32,
    word_count: u32,
    word: &str,
) {
    sqlite
        .execute(
            "INSERT INTO file_reverse_index (file,stem,offset,word) VALUES(?,?,?,?)",
            params![file_id, stem_id, word_count, word],
        )
        .unwrap();
}

// Update file's last modification time.
fn update_file_mod_time(sqlite: &Connection, last_modified: &u64, path_str: &str) {
    sqlite
        .execute(
            "UPDATE monitored_file
               SET modified = ?1
               WHERE path = ?2
            ",
            params![last_modified, path_str],
        )
        .unwrap();
}

// Wipe index information for a file.
fn clear_index_for(sqlite: &Connection, file_id: u32) {
    sqlite
        .execute(
            "DELETE FROM file_reverse_index WHERE file = ?",
            params![file_id],
        )
        .unwrap();
}
