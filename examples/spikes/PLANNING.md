# Requisites for the SocketServer API:

    * `Uni`s & `Channel`s have to be fully configurable
    * Not only closures, but functions (with their full type signatures) must be accepted as processors of the messages and connection events
    * Allows `Uni`s & `Channel`s to be programmatically determined (in opposition to requiring a code change), so the Server might be built from externally provided configurations

## Some expected usage examples

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