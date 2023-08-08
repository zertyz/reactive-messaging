# `reactive-messaging` SocketServer API spikes

This is a top-down model of the API being developed for the `SocketServer` struct of the `reactive-messaging` crate.
This model doesn't do anything -- Its only purpose is to prove the API is compilable and the flow of information works, as the API tends to be complicated as lots of Generic Programming & Generic Parameters are expected.

# Requirements

    * Don't abuse the generics. Generic Programming makes the type declarations hard to read. Whenever possible, the generic parameters should be minimized (creating new traits, enums or structs for that solo purpose is acceptable). But, keep in mind: speed is our primary goal, so abstractions that prevent the compiler from optimizing the server code should be avoided. Dynamic dispatching is acceptable if it only impacts the single call to `start()` the server;
    * `Uni`s & `Channel`s -- which comes from the `reactive-mutiny` library -- have to be fully configurable. The sources here provide a model for those components;
    * Not only closures, but functions (with their full type signatures) must be accepted as processors of the messages and connection events;
    * Allows `Uni`s & `Channel`s to be programmatically determined (in opposition to requiring a code change), so the Server might be built from externally provided configurations. As a recap, Uni channels may be zero-copy or movable, in regard to their optimizations on how to transfer each received message from the producers to the consumer Stream;
    * The library is designed for real-time processing, working with both local and over-the-network sockets, so it must be as fast as possible, allowing the same nice code optimizations & zero-cost abstractions used by `reactive-mutiny`.
    * There is a `ConstConfig` struct that contains definitions of several const options that will be used in the final implementation (specifying the retrying logic, the IO buffer size hints, etc). None of those is implemented in the `ConstConfig`, but it should be kept as a `usize` number.

## Some expected usage examples

This spike contains references to `RemoteMessages` and `LocalMessages`.
These are the message types originated in the Client and Server, respectively, that will be serialized/deserialized somehow to be sent over the network.
For all purposes in this spike, consider them to be any Rust object.
In the example code in `main.rs`, simple u32/f64 numbers are used.

Here is a simplification of what the final API might look like:

```rust
const CONFIG = the chosen Uni type;
type ProcessorChannelType = Channel::<RemoteMessages>;
type ProcessorUniType = Uni::<RemoteMessages, ProcessorChannelType>;
type SenderChannelType = Channel::<LocalMessages>;
type ServerType = SocketServer::<CONFIG, ProcessorUniType, SenderChannelType>;
type StreamItemType = ServerType::StreamItemType;
let server: trait = ServerType::new();
```

1. Built-in closures, with omited types:

    ```rust
        server.start(
            |_| the connection events callback,
            |_| processor 
        )
    ```

2. Closures with types
    ```rust
        server.start(
            |_: ServerType::ConnectionEventsType| the connection events callback,
            |_: ServerType::StreamItemType| processor 
        )
    ```

3. External functions
    ```rust
        server.start(connection_events_handler, processor);
        fn connection_events_handler(_event: ConnectionEventType) {}
        fn processor(client_messages_stream: impl Iterator<Item=StreamItemType>) -> impl Iterator<Item=LocalMessages> {
            client_messages_stream.map(|payload| *payload as f64)
        }
    ```

4. Generic external functions
    ```rust
        fn connection_events_handler<ConnectionEventType: GenericChannel>(_event: ConnectionEventType) {}
        fn processor<StreamItemType: Borrow<RemoteMessages>>(client_messages_stream: impl Iterator<Item=StreamItemType>) -> impl Iterator<Item=LocalMessages> {
            client_messages_stream.map(|payload| *payload.borrow() as f64)
        }

    ```