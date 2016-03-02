use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::AtomicBool;
use std::{ptr, mem};
use std::thread::{self, Thread};

use mem::epoch::{self, Atomic, Owned, Shared, Guard};

const ARRAY_SIZE: usize = 0b100000; // 32
const IND_MASK: u64 = 0b011111; // 31
const SHIFT_BITS: u32 = 5; /// number of bits used in local array index

type Bitarray = i32;

#[inline(always)]
fn get_index(h: u64) -> usize { (h & IND_MASK) as usize }

#[inline(always)]
fn lower_hash(h: u64) -> u64 { h >> SHIFT_BITS }

#[inline(always)]
fn get_active(b: Bitarray, i: usize) -> bool { (b & (1 << i)) != 0 }

enum NodeType<K, V> {
    Elem(Atomic<ElemNode<K, V>>),
    Array(Atomic<ArrayNode<K, V>>),
    Null,
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

    pub fn node_lookup(&self, h: u64, g: &Guard) -> bool {
        let ind = get_index(h);
        if get_active(self.bits, ind) {
            match self.ptrs[ind] {
                NodeType::Elem(ref ptr) => true,
                NodeType::Array(ref ptr) => {
                    ptr.load(Acquire, g).unwrap().node_lookup(lower_hash(h), g)
                },
                NodeType::Null => false
            }
        }
        else { false }
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
