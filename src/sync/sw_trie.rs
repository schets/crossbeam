use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::AtomicBool;
use std::{ptr, mem};
use std::thread::{self, Thread};

use mem::epoch::{self, Atomic, Owned, Shared};

static ARRAY_SIZE: i32 = 32;
type Bitarray = i32;

enum NodeType<K, V> {
    Atomic<ElemNode<K, V>>,
    Atomic<ArrayNode<K, V>>,
}

struct ElemNode<K, V> {
    hash: u64,
    key: K,
    val: V,
}

struct ArrayNode<K, V> {
    bits: Bitarray,
    ptrs: [NodeType, ARRAY_SIZE];
}

struct Table<K, V> {
    Atomic<ArrayNode<K, V>> root;
}

impl<K, V> Table<K, V> {

}
