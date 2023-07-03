# reactive-messaging

Provides reactive client/server communications focused on high performance -- through sockets, shared mem IPCs and other goodies

The distinct features of this library are:
  - a through protocol modeling, with the help of Rust's powerful `enum`s;
  - ease of logic decoupling by using reactive `Stream`s for the client / server logic.


# Taste it

Take a look at the ping-pong game in `example/`, observing the following:
  - How the ping-pong game logic is shared among the client and server;
  - How the protocol is modeled;
  - How decoupled and testable those components are;
  - How straight-forward it is to create servers & clients when all the pieces are done. 


# pre-alpha status
Research & core development is just done:
  - The main network loop for both client & server is complete & fully optimized;
  - A nice message model has been elaborated to model protocols, with a fully working `example/`;
  - The most performant reactive library & channels has been picked up, as determined by `benches/`;
  - Only textual socket protocols are supported by now -- binary & shared mem IPCs to be added later;
  - The API is not complete -- only "responsive" communications for `Stream`s that generate non-fallible & non-future elements are supported by now.