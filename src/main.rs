extern crate notify;

use notify::{Watcher, RecursiveMode, watcher};
use std::sync::mpsc::channel;
use std::time::Duration;

fn main() {
  let (tx, rx) = channel();
  let mut watcher = watcher(tx, Duration::from_secs(10)).unwrap();
  watcher.watch(".", RecursiveMode::Recursive).unwrap();
  loop {
    match rx.recv() {
      Ok(event) => println!("{:?}", event),
      Err(e) => println!("watch error: {:?}", e),
    }
  }
}
