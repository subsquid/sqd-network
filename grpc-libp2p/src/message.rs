use libp2p::PeerId;
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
};

pub trait MsgContent: Sized + Send + Debug + 'static {
    fn new(size: usize) -> Self;
    fn as_slice(&self) -> &[u8];
    fn as_mut_slice(&mut self) -> &mut [u8];
}

impl MsgContent for Box<[u8]> {
    fn new(size: usize) -> Self {
        vec![0; size].into_boxed_slice()
    }

    fn as_slice(&self) -> &[u8] {
        self.deref()
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        self.deref_mut()
    }
}

impl MsgContent for Vec<u8> {
    fn new(size: usize) -> Self {
        vec![0; size]
    }

    fn as_slice(&self) -> &[u8] {
        self.as_slice()
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

#[derive(Debug)]
pub struct Message<T: MsgContent> {
    pub peer_id: PeerId,
    pub content: T,
}
