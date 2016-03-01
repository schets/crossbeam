use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::AtomicBool;
use std::{ptr, mem};
use std::thread::{self, Thread};

use mem::epoch::{self, Atomic, Owned, Shared};

struct Node<K, V> {
    key: K,
    val: V,
    hash: u64,
    Atomic<Node<K, V>> next_iter,
    Atomic<Node<K, V>>
}

struct Table<K, V> {

}
