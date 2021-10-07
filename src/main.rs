extern crate dirs;
extern crate notify;

use gjson;
use notify::{Watcher, RecursiveMode, watcher};
use std::fs;
use std::path::Path;
use std::sync::mpsc::channel;
use std::time::Duration;

fn main() {
  let mut config_path = dirs::config_dir()
    .expect("Can't access configuration folder.");
  config_path.push("intern.json");

  let config_file = fs::read_to_string(config_path)
    .expect("Unable to read configuration file.");
  let config = gjson::parse(&config_file);
  let (tx, rx) = channel();
  let check_period = config.get("period").u64();
  let mut watcher = watcher(tx, Duration::from_secs(check_period)).unwrap();

  for folder in config.get("folder").array() {
    let recurse = folder.get("recurse").bool();
    let mode = if recurse {
      RecursiveMode::Recursive
    } else {
      RecursiveMode::NonRecursive
    };
    let folder_name = folder.get("name");
    let path = folder_name.str();

    process_folder(path, recurse);
    watcher.watch(path, mode).unwrap();
  }

  loop {
    match rx.recv() {
      Ok(event) => println!("{:?}", event),
      Err(e) => println!("watch error: {:?}", e),
    }
  }
}

fn process_folder(path: &str, recursive: bool) {
  let dir = Path::new(path);

  if dir.is_dir() {
    for entry in fs::read_dir(dir).expect("Cannot read directory") {
      let entry = entry.expect("No entry");
      let metadata = fs::metadata(entry.path()).unwrap();
      let last_modified = metadata.modified().unwrap();

      if recursive && entry.path().is_dir() {
        process_folder(entry.path().to_str().unwrap(), recursive);
      } else {
        println!("{}, {:?}", path, last_modified);
      }
    }
  }
}
