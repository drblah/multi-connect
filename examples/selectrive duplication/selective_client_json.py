import socket
import json

message = { "InterfaceState": {
    "interface_name": "veth1",
    "enabled": True
}}


soc = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
soc.bind(("0.0.0.0", 3333))

encoded_message = json.dumps(message).encode()
print(encoded_message)

soc.sendto(encoded_message, ("172.16.200.2", 1234))