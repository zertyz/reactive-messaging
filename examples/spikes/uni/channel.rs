/// Models a `Uni::channel` from `reactive-mutiny`

use std::{marker::PhantomData, sync::Arc};


pub trait GenericChannel {
    type DerivedType;
}


pub struct ChannelAtomic<MsgType> {
    pub _phantom: PhantomData<MsgType>
}
impl<MsgType> ChannelAtomic<MsgType> {}

impl<MsgType> GenericChannel for
ChannelAtomic<MsgType> {
    type DerivedType = Arc<MsgType>;
}


pub struct ChannelFullSync<MsgType> {
    pub _phantom: PhantomData<MsgType>
}
impl<MsgType> ChannelFullSync<MsgType> {}

impl<MsgType> GenericChannel for
ChannelFullSync<MsgType> {
    type DerivedType = MsgType;
}