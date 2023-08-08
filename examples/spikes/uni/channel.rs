/// Models a `Uni::channel` from `reactive-mutiny`.
/// A `Channel` may be a "Multiple producers & Multiple consumer" bounded queue, for instance.
/// It will provide means to receive messages and to send them to `Streams`, although this part is not modeled here.

use std::{marker::PhantomData, sync::Arc};


pub trait GenericChannel<const BUFFER_SIZE: usize> {
    const BUFFER_SIZE: usize;
    type ItemType;
    type DerivedType;
}


pub struct ChannelZeroCopy<const BUFFER_SIZE: usize, MsgType> {
    pub _phantom: PhantomData<MsgType>
}
impl<const BUFFER_SIZE: usize, MsgType> ChannelZeroCopy<BUFFER_SIZE, MsgType> {}

impl<const BUFFER_SIZE: usize,
     MsgType>
GenericChannel<BUFFER_SIZE> for
ChannelZeroCopy<BUFFER_SIZE, MsgType> {
    const BUFFER_SIZE: usize = BUFFER_SIZE;
    type ItemType            = MsgType;
    type DerivedType         = Arc<MsgType>;
}


pub struct ChannelMove<const BUFFER_SIZE: usize, MsgType> {
    pub _phantom: PhantomData<MsgType>
}
impl<const BUFFER_SIZE: usize, MsgType> ChannelMove<BUFFER_SIZE, MsgType> {}

impl<const BUFFER_SIZE: usize, MsgType> GenericChannel<BUFFER_SIZE> for
ChannelMove<BUFFER_SIZE, MsgType> {
    const BUFFER_SIZE: usize = BUFFER_SIZE;
    type ItemType            = MsgType;
    type DerivedType         = MsgType;
}