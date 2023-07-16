To run this nice example, full of good patterns, proceed as the following:

  1. Build the examples with `cd ..; RUSTFLAGS="-C target-cpu=native" cargo test --release --no-run; cd -`
  2. Run `sudo sync; ../target/release/examples/server & sleep 3; ../target/release/examples/client; fg`
  3. Wait for some seconds for the ping-pong matches to finish or up to 3 minutes if your computer is too slow
  4. Watch out the "Game Over" and "Disconnected" messages, with some metrics and messages per seconds summaries
  5. On my development machine, I get ~900k back-and-forth messages per second (constrained by the loopback ping latency)
     and the Server process only consumes ~25% of one of the core threads in Linux.
     FreeBSD is able to achieve lower latencies for this loopback test.
