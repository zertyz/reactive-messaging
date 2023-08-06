/// Models a Uni from `reactive-mutiny`

use std::marker::PhantomData;

use super::channel::GenericChannel;


pub struct Uni<MsgType,
               ChannelType: GenericChannel> {
    pub _phantom: PhantomData<(MsgType,ChannelType)>
}

impl<MsgType,
     ChannelType: GenericChannel>
Uni<MsgType, ChannelType> {}

pub trait GenericUni {
    type DerivedType;
}
impl<MsgType,
     ChannelType: GenericChannel> GenericUni for
Uni<MsgType, ChannelType> {
    type DerivedType = <ChannelType as GenericChannel>::DerivedType;
}

pub struct ConcreteIterator<Item> {
    _phantom_data: PhantomData<Item>
}
impl<Item>
Iterator for
ConcreteIterator<Item> {
    type Item=Item;
    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}