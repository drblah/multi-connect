#!/bin/bash

set -e

##
# Based on https://ops.tips/blog/using-network-namespaces-and-bridge-to-isolate-servers/
##
# Create namespaces
ip netns add host1
ip netns add host2

# Create two veth pairs
ip link add veth1 type veth peer name br-veth1
ip link add veth2 type veth peer name br-veth2
ip link add veth3 type veth peer name br-veth3

# Associate the veth pairs with the namespaces
ip link set veth1 netns host1
ip link set veth2 netns host1

ip link set veth3 netns host2


# Assign IPs
ip netns exec host1 \
  ip addr add 172.16.200.2/24 dev veth1

ip netns exec host1 \
  ip addr add 172.16.200.3/24 dev veth2

ip netns exec host2 \
  ip addr add 172.16.200.4/24 dev veth3

# Create bridge
ip link add name mptun_bridge type bridge
ip link set mptun_bridge up

# Bring up all interfaces
ip link set br-veth1 up
ip link set br-veth2 up
ip link set br-veth3 up

ip netns exec host1 \
  ip link set veth1 up

ip netns exec host1 \
  ip link set veth2 up

ip netns exec host2 \
  ip link set veth3 up


# Add br-veth* to the bridge
ip link set br-veth1 master mptun_bridge
ip link set br-veth2 master mptun_bridge
ip link set br-veth3 master mptun_bridge

# Assign address to bridge
ip addr add 172.16.200.1/24 brd + dev mptun_bridge

# Start interactive consoles for each namespace

tmux \
	new-session  "ip netns exec host1 bash" \; \
	split-window "ip netns exec host2 bash" \; \
	select-layout even-vertical

P1=$!


wait $P1

## Clean up the bridge and namespaces
ip netns delete host1
ip netns delete host2
ip link delete dev mptun_bridge
