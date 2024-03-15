use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
};

use derivative::Derivative;

use crate::PeerId;

pub trait MsgContent: Sized + Send + Debug + 'static {
    fn new(size: usize) -> Self;
    fn as_slice(&self) -> &[u8];
    fn as_mut_slice(&mut self) -> &mut [u8];
    fn to_vec(self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
    fn from_vec(vec: Vec<u8>) -> Self {
        let mut content = Self::new(vec.len());
        content.as_mut_slice().copy_from_slice(vec.as_slice());
        content
    }
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

    fn from_vec(vec: Vec<u8>) -> Self {
        vec.into_boxed_slice()
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

    fn to_vec(self) -> Vec<u8> {
        self
    }

    fn from_vec(vec: Vec<u8>) -> Self {
        vec
    }
}

#[derive(Derivative, Debug)]
pub struct Message<T: MsgContent> {
    // None for outgoing broadcast messages, Some for others
    pub peer_id: Option<PeerId>,
    // None for direct messages, Some for broadcast messages
    pub topic: Option<String>,
    #[derivative(Debug = "ignore")]
    pub content: T,
}
