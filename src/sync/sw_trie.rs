use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::AtomicBool;
use std::{ptr, mem};
use std::thread::{self, Thread};

use mem::epoch::{self, Atomic, Owned, Shared};

const ARRAY_SIZE: usize = 32;
type Bitarray = i32;

enum NodeType<K, V> {
    Elem(Atomic<ElemNode<K, V>>),
    Array(Atomic<ArrayNode<K, V>>),
}

struct ElemNode<K, V> {
    next: Atomic<ElemNode<K, V>>,
    hash: u64,
    key: K,
    val: V,
}

struct ArrayNode<K, V> {
    bits: Bitarray,
    ptrs: [NodeType<K, V>; ARRAY_SIZE],
}

impl<K, V> ArrayNode<K, V> {
    pub fn new() -> ArrayNode<K, V> {
        let mut a = ArrayNode {
            bits: 0,
            ptrs: unsafe { mem::uninitialized() },
        };
        for i in 0..ARRAY_SIZE {
            a.ptrs[i] = NodeType::Elem(Atomic::null());
        }
        a
    }
}

struct Table<K, V> {
    root: Atomic<ArrayNode<K, V>>,
}

impl<K, V> Table<K, V> {
    pub fn new() -> Table<K, V> {
        Table {
            root: Atomic::new(ArrayNode::new())
        }
    }
}
