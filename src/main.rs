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
use rusqlite::{params, Connection};
use rust_stemmers::{Algorithm, Stemmer};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, UNIX_EPOCH};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug)]
struct MonitoredFile {
    id: u32,
    modified: u64,
}

#[derive(Debug)]
struct WordStem {
    id: u32,
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

    enforce_data_model(&sqlite);
    for folder in config.get("folder").array() {
        let recurse = folder.get("recurse").bool();
        let mode = if recurse {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        let folder_name = folder.get("name");
        let path = folder_name.str();

        process_folder(&sqlite, path, recurse, &punc, &acc, &stem);
        watcher.watch(path, mode).unwrap();
    }

    loop {
        match rx.recv() {
            Ok(event) => match event {
                Chmod(event) => println!("{:?}", event),
                Create(epath) => {
                    let path = epath.to_str().unwrap();
                    let metadata = fs::metadata(path).unwrap();
                    let last_modified = metadata
                        .modified()
                        .unwrap()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    process_file(&sqlite, path, &punc, &acc, &stem, last_modified);
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
) {
    let dir = Path::new(path);

    if dir.is_dir() {
        for entry in fs::read_dir(dir).expect("Cannot read directory") {
            let entry = entry.expect("No entry");
            let metadata = fs::metadata(entry.path()).unwrap();
            let last_modified = metadata
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let entry_path = entry.path();
            let path_str = entry_path.to_str().unwrap();

            if recursive && entry.path().is_dir() {
                process_folder(sqlite, path_str, recursive, punc, acc, stem);
            } else if entry.path().is_dir() {
                // Should probably do something, but I don't know what, yet.
            } else {
                process_file(sqlite, path_str, punc, acc, stem, last_modified);
            }
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
) {
    let mut fileq = sqlite
        .prepare("SELECT id, modified FROM monitored_file where path = ?")
        .unwrap();
    let mod_times = fileq
        .query_map(params![path_str], |row| {
            Ok(MonitoredFile {
                id: row.get(0).unwrap(),
                modified: row.get(1).unwrap(),
            })
        })
        .unwrap();
    let mod_time = mod_times.last();

    match mod_time {
        Some(some_mod) => {
            let mtime = some_mod.unwrap();
            if mtime.modified < last_modified {
                sqlite
                    .execute(
                        "UPDATE monitored_file
                           SET modified = ?1
                           WHERE path = ?2
                        ",
                        params![last_modified, path_str],
                    )
                    .unwrap();
                index_file(sqlite, path_str, mtime.id, punc, acc, stem, last_modified);
            }
        }
        None => {
            sqlite
                .execute(
                    "INSERT
                       INTO monitored_file (path, modified)
                       VALUES (?, ?)
                    ",
                    params![path_str, last_modified],
                )
                .unwrap();

            let mod_times = fileq
                .query_map(params![path_str], |row| {
                    Ok(MonitoredFile {
                        id: row.get(0).unwrap(),
                        modified: row.get(1).unwrap(),
                    })
                })
                .unwrap();
            let mod_time = mod_times.last().unwrap().unwrap();

            index_file(
                sqlite,
                path_str,
                mod_time.id,
                punc,
                acc,
                stem,
                last_modified,
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
    acc: &Regex,
    stem: &Stemmer,
    last_modified: u64,
) {
    let text = fs::read_to_string(path).unwrap();
    let alpha_only = punc.replace_all(&text, " ");
    let space_split = alpha_only.split_whitespace();
    let mut word_count = 0;

    // Delete any existing index.
    if file_id > 0 {
        sqlite
            .execute(
                "DELETE FROM file_reverse_index WHERE file = ?",
                params![file_id],
            )
            .unwrap();
    } else {
        sqlite
            .execute(
                "INSERT
                   INTO monitored_file (path,modified)
                   VALUES (?,?)
                ",
                params![path, last_modified],
            )
            .unwrap();

        let mut fileq = sqlite
            .prepare("SELECT id, modified FROM monitored_file where path = ?")
            .unwrap();
        let mod_times = fileq
            .query_map(params![path], |row| {
                Ok(MonitoredFile {
                    id: row.get(0).unwrap(),
                    modified: row.get(1).unwrap(),
                })
            })
            .unwrap();
        let mod_time = mod_times.last().unwrap().unwrap();

        file_id = mod_time.id;
    }

    space_split.filter(|w| !punc.is_match(w)).for_each(|word| {
        let nfd = word.to_string().nfd().collect::<String>();
        let no_acc = acc.replace_all(&nfd, "").to_lowercase();
        let stem = stem.stem(&no_acc);
        let mut stemq = sqlite
            .prepare("SELECT id FROM word_stem where stem = ?")
            .unwrap();
        let stems = stemq
            .query_map(params![stem], |row| {
                Ok(WordStem {
                    id: row.get(0).unwrap(),
                })
            })
            .unwrap();
        let stem_id: u32;
        let stem_row = stems.last();

        // Create a stem if necessary.  Otherwise, use its ID.
        match stem_row {
            Some(stem) => stem_id = stem.unwrap().id,
            None => {
                sqlite
                    .execute("INSERT INTO word_stem (stem) VALUES(?)", params![stem])
                    .unwrap();

                let stems = stemq
                    .query_map(params![stem], |row| {
                        Ok(WordStem {
                            id: row.get(0).unwrap(),
                        })
                    })
                    .unwrap();
                stem_id = stems.last().unwrap().unwrap().id;
            }
        }

        // Add the next word-tuple to the index.
        sqlite
            .execute(
                "INSERT INTO file_reverse_index (file,stem,offset,word) VALUES(?,?,?,?)",
                params![file_id, stem_id, word_count, word],
            )
            .unwrap();
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

fn find_paths() -> (PathBuf, PathBuf) {
    let mut config_path = dirs::config_dir().expect("Can't access configuration folder.");
    config_path.push("intern");
    config_path.push("intern.json");

    let mut db_path = dirs::config_dir().expect("Can't access configuration folder.");
    db_path.push("intern");
    db_path.push("intern.sqlite3");

    (config_path, db_path)
}
