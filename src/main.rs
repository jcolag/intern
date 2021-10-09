extern crate dirs;
extern crate notify;
extern crate rusqlite;

use notify::{watcher, RecursiveMode, Watcher};
use rusqlite::{params, Connection};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, UNIX_EPOCH};

fn main() {
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

        process_folder(&sqlite, path, recurse);
        watcher.watch(path, mode).unwrap();
    }

    loop {
        match rx.recv() {
            Ok(event) => println!("{:?}", event),
            Err(e) => println!("watch error: {:?}", e),
        }
    }
}

// Iterate through the files in the folder, adding or indexing any files
// that are new or updated since our last run.
fn process_folder(sqlite: &Connection, path: &str, recursive: bool) {
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
                process_folder(sqlite, path_str, recursive);
            } else {
                let mut stmt = sqlite
                    .prepare("SELECT modified FROM monitored_file where path = ?")
                    .unwrap();
                let mut rows = stmt.query(params![path]).unwrap();
                let mut mod_times = Vec::<u64>::new();
                while let Some(row) = rows.next().unwrap() {
                    mod_times.push(row.get(0).unwrap());
                }
                if mod_times.is_empty() {
                    sqlite
                        .execute(
                            "INSERT INTO monitored_file (path, modified) VALUES (?1, ?2)",
                            params![path_str, last_modified],
                        )
                        .unwrap();
                } else if mod_times[0] < last_modified {
                    sqlite
                        .execute(
                            "UPDATE monitored_file SET modified = ?1 WHERE path = ?2",
                            params![last_modified, path_str],
                        )
                        .unwrap();
                }
            }
        }
    }
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
