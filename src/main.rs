extern crate dirs;
extern crate notify;

use gjson;
use notify::{Watcher, RecursiveMode, watcher};
use std::fs;
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
    let recurse = if folder.get("recurse").bool() {
      RecursiveMode::Recursive
    } else {
      RecursiveMode::NonRecursive
    };

    watcher.watch(folder.get("name").str(), recurse).unwrap();
  }

  loop {
    match rx.recv() {
      Ok(event) => println!("{:?}", event),
      Err(e) => println!("watch error: {:?}", e),
    }
  }
}
