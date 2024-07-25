# mpconn
multi-connect is a flexible multi connectivity tunneling tool. 

## What is multi connectivity?
Multi connectivity is the act of using two or more network paths in order to improve reliability and/or latency. Multi connectivity is especially useful when operating over an inherently unreliable physical layer such as wireless networks.
The simplest form of multi connectivity is packet duplication, where each network packet is duplicated and a copy is transmitted over each available physical link. By doing this, it is possible to take advantage of the fact that negative network conditions are often uncorrelated across the various network layers.

## Installation
To install mpconn, you need the Rust nightly toolchain with Cargo and libpcap.

### Ubuntu
```
apt-get install libpcap-dev
```
The easiest way to get Rust is to install it using rustup from https://rustup.rs/

Then set up the nightly toolchain:
```
rustup install nightly
rustup default nightly
```

## Usage
Each endpoint needs a host configuration. This configuration involves one or more remote endpoints to tunnel between,
which Remote transport protocol to use and which layer to tunnel. Examples can be found in the test_tools directory
along with bash scripts for setting up network namespace based testing environments.
A multi-connect network consists of one server, and one or more client(s).

### How to start the server:
```
./server --config <host-config>.json
```

### How to start the client
```
./server --config <host-config>.json
```
To get additional logging run it with the RUST_LOG environment
variable `RUST_LOG=debug ./client --config <host-config>.json`
