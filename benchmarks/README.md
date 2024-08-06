# Objective

Since the performance is a key requirement of this crate, With these benchmarks, we aim to track the performance, over time, of some real-ish world usage scenarios -- so that we can be sure no performance regressions are introduced either by our sources, Rust versions, operating system, etc.

With this we also have a glimpse on how this crate behaves on different hardware.

# Guide

Here you will find the following directories:
  * `<computer_name>`: Each directory have a `machine.info` file describing the hardware + a bunch of other files with measurement data -- detailed bellow.

The methodology for the new benchmarks are as follows:
  * Identification: inside each directory, the benchmarks will be named as: `<commit-hash>-<rust-version>-<kernel-version>.txt`
  * Compilation: `export RUSTFLAGS="-C target-cpu=native"; cargo test --release`
  * Execution: with a freshly booted machine, without any other running processes, the following is executed:
```
clear; sudo sync; ./target/release/examples/composite_protocol_stacking_server & sleep 3; ./target/release/examples/composite_protocol_stacking_client; fg
```
    NOTE 1: edit the examples/composite_protocol_stacking_client/protocol_processor.rs file and make sure `score_limit` is set to 15000
    NOTE 2: edit the examples/composite_protocol_stacking_client/main.rs file to adjust the number of concurrent clients. You should find the number which gives the most messages per second combined.
  * Summarization: Find the average messages per second and put that on the file in this form: `avg_msgs_per_sec * number_of_clients = total_messages_per_second`. Register, at least, 1 `number_of_clients` bellow the optimal value and another one above it.
