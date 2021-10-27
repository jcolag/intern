extern crate dirs;
extern crate notify;
extern crate regex;
extern crate rusqlite;
extern crate rust_stemmers;
extern crate unicode_normalization;

use gitignore;
use mio::net::TcpListener;
use mio::{Events, Interest, Poll, Token};
use notify::DebouncedEvent::{
    Chmod, Create, Error, NoticeRemove, NoticeWrite, Remove, Rename, Rescan,
    Write as NotifyWrite,
};
use notify::{watcher, RecursiveMode, Watcher};
use regex::Regex;
use rusqlite::{params, params_from_iter, Connection, Statement};
use rust_stemmers::{Algorithm, Stemmer};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, io, str};
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

#[derive(Debug)]
struct IndexTuple {
    id: u32,
    file: u32,
    stem: u32,
    offset: u32,
    word: String,
}

#[derive(Debug)]
struct IgnoreFile<'a> {
    path: String,
    file: gitignore::File<'a>,
}

#[derive(Debug)]
struct SearchResult {
    path: String,
    word: String,
    stem: u32,
    offset: u32,
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
    let server_addr = "0.0.0.0:48813".parse().unwrap();
    let mut server = TcpListener::bind(server_addr).unwrap();
    let mut server_poll = Poll::new().unwrap();
    let mut events = Events::with_capacity(1024);
    let server_token: Token = Token(0);

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
        let ignoregit = Path::new(path).join(".gitignore");
        let ignorehg = Path::new(path).join(".gitignore");
        let ignores = if ignoregit.exists() {
            gitignore::File::new(&ignoregit)
        } else {
            // This will produce an error, if neither file exists.
            gitignore::File::new(&ignorehg)
        };

        process_folder(
            &sqlite,
            path,
            recurse,
            &punc,
            &acc,
            &stem,
            &mut fileq,
            &Vec::<PathBuf>::new(),
        );
        match &ignores {
            Ok(ignore) => {
                // Either un-watching or ignore status doesn't work as
                // expected, so we flip the logic, only watching
                // non-ignored files.
                watcher.watch(path, RecursiveMode::NonRecursive).unwrap();
                ignore.included_files().into_iter().for_each(|files| {
                    files.into_iter().for_each(|include| {
                        watcher
                            .watch(
                                Path::new(include.to_str().unwrap()),
                                RecursiveMode::NonRecursive,
                            )
                            .unwrap();
                    });
                });
            }
            // Not an error; just no ignore file
            Err(_) => watcher.watch(path, mode).unwrap(),
        }
    }

    server_poll
        .registry()
        .register(&mut server, server_token, Interest::READABLE)
        .unwrap();
    match SystemTime::now().duration_since(start) {
        Ok(n) => println!("{} seconds", n.as_secs()),
        Err(_) => panic!("Something bad"),
    }

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => match event {
                Chmod(event) => println!("chmod {:?}", event),
                Create(epath) => {
                    let path = epath.to_str().unwrap();
                    let last_modified = file_mod_time(path);

                    if path.contains(".git")
                        || path.contains(".hg")
                        || path.ends_with(".svg")
                    {
                        continue;
                    }

                    watcher.watch(path, RecursiveMode::NonRecursive).unwrap();
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
                Error(event, _path) => println!("error {:?}", event),
                NoticeRemove(event) => println!("nremove {:?}", event),
                NoticeWrite(event) => println!("write {:?}", event),
                Remove(event) => println!("remove {:?}", event),
                Rename(old, new) => println!("{:?} => {:?}", old, new),
                Rescan => println!("rescan {:?}", event),
                NotifyWrite(path) => println!("nwrite {:?}", path),
            },
            Err(e) => {
                if e != std::sync::mpsc::RecvTimeoutError::Timeout {
                    println!("watch error: {:#?}", e);
                }
            }
        }

        server_poll
            .poll(&mut events, Some(Duration::from_millis(100)))
            .unwrap();
        handle_queries(
            &sqlite,
            &events,
            &server,
            &server_poll,
            server_token,
            &punc,
            &acc,
            &stem,
        );
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
    ignored: &Vec<PathBuf>,
) {
    let dir = Path::new(path);
    let filename = dir.file_name().unwrap();
    let gitignore = dir.join(".gitignore");
    let hgignore = dir.join(".hgignore");
    let mut ignores = Vec::<IgnoreFile>::new();

    if !dir.is_dir() || filename == ".git" || filename == ".hg" {
        return;
    }

    ignored.iter().for_each(|i| {
        ignores.push(IgnoreFile {
            path: String::from(i.as_path().to_str().unwrap()),
            file: gitignore::File::new(&i).unwrap(),
        });
    });

    if gitignore.exists() {
        ignores.push(IgnoreFile {
            path: String::from(gitignore.as_path().to_str().unwrap()),
            file: gitignore::File::new(&gitignore).unwrap(),
        });
    }

    if hgignore.exists() {
        ignores.push(IgnoreFile {
            path: String::from(hgignore.as_path().to_str().unwrap()),
            file: gitignore::File::new(&hgignore).unwrap(),
        });
    }

    for entry in fs::read_dir(dir).expect("Cannot read directory") {
        let entry = entry.expect("No entry");
        let last_modified = file_mod_time(entry.path().to_str().unwrap());
        let entry_path = entry.path();
        let path_str = entry_path.to_str().unwrap();

        if recursive && entry.path().is_dir() {
            process_folder(
                sqlite,
                path_str,
                recursive,
                punc,
                acc,
                stem,
                fileq,
                &ignores.iter().map(|i| PathBuf::from(&i.path)).collect(),
            );
        } else if entry.path().is_dir() {
            // Should probably do something, but for now, it's just to prevent
            // directories from falling through to be managed as normal files.
        } else {
            let mut ignore = false;
            for i in 0..ignores.len() {
                ignore =
                    ignore || ignores[i].file.is_excluded(Path::new(&path_str)).unwrap();
            }

            if !ignore {
                process_file(sqlite, path_str, punc, acc, stem, last_modified, fileq);
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
    let text = fs::read_to_string(path).unwrap_or("".to_string());
    let alpha_only = punc.replace_all(&text, " ");
    let mut space_split = alpha_only.split_whitespace();
    let mut word_count = 0;
    let mut all_stems = select_all_stems(sqlite);
    let mut new_stems = Vec::<String>::new();
    let mut new_index_tuples = Vec::<IndexTuple>::new();

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
        let tuple = IndexTuple {
            id: 0,
            file: file_id,
            stem: stem_id,
            offset: word_count,
            word: word.to_string(),
        };
        new_index_tuples.push(tuple);
        word_count += 1;
    });

    insert_bulk_word_tuples(sqlite, new_index_tuples);
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
fn stem_word(word: &str, accents: &Regex, stem: &Stemmer) -> String {
    let nfd = word.to_string().nfd().collect::<String>();
    let no_accents = accents.replace_all(&nfd, "").to_lowercase();
    stem.stem(&no_accents).trim().to_string()
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
fn select_all_stems(sqlite: &Connection) -> HashMap<String, u32> {
    let mut result = HashMap::new();
    let mut stemq = sqlite.prepare("SELECT id, stem FROM word_stem").unwrap();
    let stem_iter = stemq
        .query_map([], |row| {
            Ok(WordStem {
                id: row.get(0).unwrap(),
                stem: row.get(1).unwrap(),
            })
        })
        .unwrap();

    for stem in stem_iter {
        let raw_stem = stem.unwrap();

        result.insert(raw_stem.stem.to_string(), raw_stem.id);
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
fn insert_bulk_stems(sqlite: &Connection, stems: Vec<String>) -> HashMap<String, u32> {
    let placeholders = stems.iter().map(|_| "(?)").collect::<Vec<_>>().join(", ");
    let query = format!("INSERT INTO word_stem (stem) VALUES {}", placeholders);

    if stems.is_empty() {
        return select_all_stems(sqlite);
    }

    sqlite
        .execute(&query, params_from_iter(stems.iter()))
        .unwrap();
    select_all_stems(sqlite)
}

// Index a file's file-stem-position tuples.
fn insert_bulk_word_tuples(sqlite: &Connection, mut words: Vec<IndexTuple>) {
    let mut remainder = Vec::<IndexTuple>::new();
    let max_values = 8192;

    if words.is_empty() {
        return;
    }

    loop {
        if words.len() > max_values {
            remainder = words.split_off(max_values);
        }

        let placeholders = words
            .iter()
            .map(|_| "(?,?,?,?)")
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "INSERT INTO file_reverse_index (file,stem,offset,word) VALUES {}",
            placeholders
        );
        let mut values = Vec::<String>::new();

        for word in words {
            values.push(word.file.to_string());
            values.push(word.stem.to_string());
            values.push(word.offset.to_string());
            values.push(word.word.to_string());
        }

        match sqlite.execute(&query, params_from_iter(values.iter())) {
            Ok(_) => (),
            Err(e) => panic!("Error:  {}", e),
        }

        words = remainder;
        remainder = Vec::<IndexTuple>::new();
        if words.is_empty() {
            break;
        }
    }
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

// Retrieve stem information from the index.
fn search_index(sqlite: &Connection, stems: Vec<WordStem>) -> Vec<SearchResult> {
    let mut result = Vec::<SearchResult>::new();
    let placeholders = stems.iter().map(|_| "(?)").collect::<Vec<_>>().join(", ");
    let query = format!(
        "SELECT f.path, i.word, i.stem, i.offset FROM file_reverse_index i JOIN monitored_file f ON f.id = i.file WHERE i.stem IN ({}) ORDER BY f.path, i.stem, i.offset",
        placeholders
    );
    let ids = stems.iter().map(|s| s.id);
    let mut stemq = sqlite.prepare(&query).unwrap();
    let index_entries = stemq
        .query_map(params_from_iter(ids), |row| {
            Ok(SearchResult {
                path: row.get(0).unwrap(),
                word: row.get(1).unwrap(),
                stem: row.get(2).unwrap(),
                offset: row.get(3).unwrap(),
            })
        })
        .unwrap();

    index_entries.for_each(|ie| result.push(ie.unwrap()));
    result
}

// Organize a list sorted by file, stem, and offset
//
// Note that some of this code is clunky, copying data back and forth
// between objects, to make sure that we don't violate Rust's ownership
// rules.
fn collate_search(
    search: Vec<SearchResult>,
    stem_ids: Vec<u32>,
) -> HashMap<String, HashMap<u32, Vec<SearchResult>>> {
    let mut result = HashMap::<String, HashMap<u32, Vec<SearchResult>>>::new();
    let mut by_stem = Vec::<SearchResult>::new();
    let mut by_file = HashMap::<u32, Vec<SearchResult>>::new();
    let mut last_stem = 0;
    let mut last_file = "";

    search.iter().for_each(|sr| {
        // We don't actually want special behavior on the first run,
        // so we fake having a previous run with these conditions.
        if last_file == "" {
            last_file = &sr.path;
        }

        if last_stem == 0 {
            last_stem = sr.stem;
        }

        // Reset the stem list when the stem or file changes.
        if sr.stem != last_stem || sr.path != last_file {
            let mut stems = Vec::<SearchResult>::new();

            by_stem.iter().for_each(|s| {
                stems.push(SearchResult {
                    path: s.path.to_string(),
                    word: s.word.to_string(),
                    stem: s.stem,
                    offset: s.offset,
                })
            });
            by_file.insert(last_stem, stems);
            by_stem = Vec::<SearchResult>::new();
            last_stem = sr.stem;
        }

        // Reset the file list when the file changes.
        if sr.path != last_file {
            let mut files = HashMap::<u32, Vec<SearchResult>>::new();
            let mut all_found = true;

            by_file.keys().for_each(|k| {
                let mut stems = Vec::<SearchResult>::new();

                by_file[&k].iter().for_each(|s| {
                    stems.push(SearchResult {
                        path: s.path.to_string(),
                        word: s.word.to_string(),
                        stem: s.stem,
                        offset: s.offset,
                    });
                });
                files.insert(*k, stems);
            });
            stem_ids
                .iter()
                .for_each(|s| all_found &= files.contains_key(s));
            if all_found {
                result.insert(last_file.to_string(), files);
            }

            by_file = HashMap::<u32, Vec<SearchResult>>::new();
            last_file = &sr.path;
        }

        by_stem.push(SearchResult {
            path: sr.path.to_string(),
            word: sr.word.to_string(),
            stem: sr.stem,
            offset: sr.offset,
        });
    });

    result
}

// Accept requests for searches and return any search results.
fn handle_queries(
    sqlite: &Connection,
    events: &Events,
    server: &TcpListener,
    server_poll: &Poll,
    server_token: Token,
    punc: &Regex,
    accents: &Regex,
    stemmer: &Stemmer,
) {
    for _event in events.iter() {
        let (mut client, _addr) = match server.accept() {
            Ok((client, _addr)) => (client, _addr),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                break;
            }
            Err(e) => {
                println!("{:?}", e);
                return;
            }
        };
        let mut buffer = [0; 4096];

        server_poll
            .registry()
            .register(
                &mut client,
                server_token,
                Interest::READABLE.add(Interest::WRITABLE),
            )
            .unwrap();
        match client.read(&mut buffer) {
            Ok(_) => {
                let query = str::from_utf8(&buffer).unwrap();
                let alpha_only = punc.replace_all(&query, " ");
                let space_split = alpha_only.split_whitespace();
                let all_stems = select_all_stems(sqlite);
                let mut new_stems = Vec::<WordStem>::new();

                space_split.filter(|w| !punc.is_match(w)).for_each(|word| {
                    let stem = stem_word(word, accents, stemmer);
                    let id = if all_stems.contains_key(&stem) {
                        all_stems[&stem]
                    } else {
                        0
                    };

                    new_stems.push(WordStem { id: id, stem: stem });
                });

                let search_results = search_index(sqlite, new_stems);
                let serps = collate_search(search_results);
                println!("{:#?}", serps);

                client.write(query.as_bytes()).unwrap();
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => println!("{:#?}", e),
        }
    }
}
