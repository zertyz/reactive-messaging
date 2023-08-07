/// Models a Uni from `reactive-mutiny`

use std::marker::PhantomData;

use super::channel::GenericChannel;


pub struct Uni<const INSTRUMENTS: usize,
               const BUFFER_SIZE: usize,
               MsgType,
               ChannelType: GenericChannel<BUFFER_SIZE>> {
    pub _phantom: PhantomData<(MsgType,ChannelType)>
}

impl<const INSTRUMENTS: usize,
     const BUFFER_SIZE: usize,
     MsgType,
     ChannelType: GenericChannel<BUFFER_SIZE>>
Uni<INSTRUMENTS, BUFFER_SIZE, MsgType, ChannelType> {}

pub trait GenericUni<const INSTRUMENTS: usize,
                     const BUFFER_SIZE: usize> {
    const INSTRUMENTS: usize;
    type ItemType;
    type UniChannelType: GenericChannel<BUFFER_SIZE>;
    type DerivedItemType;
}
impl<const INSTRUMENTS: usize,
     const BUFFER_SIZE: usize,
     MsgType,
     ChannelType: GenericChannel<BUFFER_SIZE>>
GenericUni<INSTRUMENTS, BUFFER_SIZE> for
Uni<INSTRUMENTS, BUFFER_SIZE, MsgType, ChannelType> {
    const INSTRUMENTS: usize = INSTRUMENTS;
    type ItemType            = MsgType;
    type UniChannelType      = ChannelType;
    type DerivedItemType     = <ChannelType as GenericChannel<BUFFER_SIZE>>::DerivedType;
}

pub type MessagingMutinyStream<const UNI_INSTRUMENTS: usize,
                               const PROCESSOR_BUFFER_SIZE: usize,
                               GenericUniType/*: GenericUni<UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>*/>
    = MutinyStream<UNI_INSTRUMENTS,
                   PROCESSOR_BUFFER_SIZE,
                   <GenericUniType as GenericUni<UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>>::ItemType,
                   <GenericUniType as GenericUni<UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>>::UniChannelType,
                   <GenericUniType as GenericUni<UNI_INSTRUMENTS, PROCESSOR_BUFFER_SIZE>>::DerivedItemType>;
pub struct MutinyStream<const UNI_INSTRUMENTS: usize,
                        const BUFFER_SIZE: usize,
                        Item,
                        UniChannelType: GenericChannel<BUFFER_SIZE>,
                        DerivedItem> {
    _phantom_data: PhantomData<(Item, UniChannelType, DerivedItem)>
}
impl<const UNI_INSTRUMENTS: usize,
     const BUFFER_SIZE: usize,
     Item,
     UniChannelType: GenericChannel<BUFFER_SIZE>,
     DerivedItem>
Iterator for
MutinyStream<UNI_INSTRUMENTS, BUFFER_SIZE, Item, UniChannelType, DerivedItem> {
    type Item=DerivedItem;
    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}