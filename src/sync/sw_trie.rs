use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::AtomicBool;
use std::{ptr, mem};
use std::thread::{self, Thread};

use mem::epoch::{self, Atomic, Owned, Shared, Guard};

const ARRAY_SIZE: usize = 0b100000; // 32
const IND_MASK: u64 = 0b011111; // 31
const SHIFT_BITS: u32 = 5; // number of bits used in local array index
const MAX_DEPTH: usize = 12; // Once we are in the 12th level the hash is exhausted


type Bitarray = i32;

#[inline(always)]
fn get_index(h: u64) -> usize { (h & IND_MASK) as usize }

#[inline(always)]
fn lower_hash(h: u64) -> u64 { h >> SHIFT_BITS }

#[inline(always)]
fn get_active(b: Bitarray, i: usize) -> bool { (b & (1 << i)) != 0 }

enum Node<K, V> {
    Elem{key: K, val: V, h: u64, next: Atomic<Elem<K, V>>},
    Array{bits: Bitarray, ptrs: [Atomic<Node<K, V>>; ARRAY_SIZE]},
}

fn new_ar<K, V>() -> Node<K, V> {
    let mut a = Node::Array {
        bits: 0,
        ptrs: unsafe { mem::uninitialized() },
    };
    for i in 0..ARRAY_SIZE {
        a.ptrs[i] = NodeType::Elem(Atomic::null());
    }
    a
}

fn node_lookup(node: &Node<K, V>, h: u64, g: &Guard) -> bool {
    let ind = get_index(h);
    if get_active(node.bits, ind) {
        match node.ptrs[ind] {
            NodeType::Elem(ref ptr) => true,
            NodeType::Array(ref ptr) => {
                node_lookup(&ptr.load(Acquire, g).unwrap(), lower_hash(h), g)
            },
            NodeType::Null => false
        }
    }
    else { false }
}

fn insert_node<K, V>(node: &mut Node::Array<K, V>, h: u64, k: K, v: V, g: &Guard) {
    let ind = get_index(h);
    if get_active(node.bites, ind) {

    }
    else {
        let o = Owned::new(Node::Elem{key: k, val: v, h: h, });
        node.ptrs[ind].store(o, Release, g);
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
