#!/bin/bash

set -e

delete_netns_if_exists() {
  NETNS_NAME=$1

  if ip netns list | grep -q "$NETNS_NAME"; then
    echo "Deleting netns: $NETNS_NAME"
    ip netns delete $NETNS_NAME
  fi
}

delete_dev_if_exists() {
  DEV_NAME=$1

  if ip link | grep -q "$DEV_NAME"; then
    echo "Deleting dev: $DEV_NAME"
    ip link delete dev $DEV_NAME
  fi
}

## Clean up the bridge and namespaces
clean_up() {
    
  delete_netns_if_exists host1

  sleep 0.1 # Otherwise we have a race condition where veth1 and 2 are deleted automatically when host1 is removed
  
  delete_dev_if_exists veth1
  delete_dev_if_exists veth2
  delete_dev_if_exists veth3
  delete_dev_if_exists mptun_bridge

}

clean_up

##
# Based on https://ops.tips/blog/using-network-namespaces-and-bridge-to-isolate-servers/
##
# Create namespaces
ip netns add host1

# Create two veth pairs
ip link add veth1 type veth peer name br-veth1
ip link add veth2 type veth peer name br-veth2
ip link add veth3 type veth peer name br-veth3

# Associate the veth pairs with the namespaces
ip link set veth1 netns host1
ip link set veth2 netns host1


# Assign IPs
ip netns exec host1 \
  ip addr add 172.16.200.2/24 dev veth1

ip netns exec host1 \
  ip addr add 172.16.200.3/24 dev veth2

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

ip netns exec host1 \
  tc qdisc add dev veth1 root netem latency 20ms 2ms limit 650000

ip netns exec host1 \
  tc qdisc add dev veth2 root netem latency 10ms 2ms limit 650000

tc qdisc add dev br-veth1 root netem latency 20ms 2ms limit 650000
tc qdisc add dev br-veth2 root netem latency 10ms 2ms limit 650000
  
ip link set veth3 up


# Add br-veth* to the bridge
ip link set br-veth1 master mptun_bridge
ip link set br-veth2 master mptun_bridge
ip link set br-veth3 master mptun_bridge

# Assign address to bridge
ip addr add 172.16.200.1/24 brd + dev mptun_bridge

# Start interactive consoles for each namespace

ip netns exec host1 bash
#tmux \
#	new-session  "ip netns exec host1 bash" \; \
#	split-window "ip netns exec host2 bash" \; \
#	select-layout even-vertical

P1=$!


wait $P1


clean_up
