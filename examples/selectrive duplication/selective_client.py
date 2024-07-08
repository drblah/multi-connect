import socket

class Message:
    def __init__(self, interface_name, enabled):
        self.interface_name = interface_name
        self.enabled = enabled

    def as_bytes(self):
        header = [3, 0, 0, 0]

        # interface_name length as 64 bits integer
        name_length = len(self.interface_name)
        name_length_bytes = name_length.to_bytes(8, byteorder='little')

        name_bytes = self.interface_name.encode('utf-8')
        enabled_byte = [1 if self.enabled else 0]

        return bytes(header) + name_length_bytes + name_bytes + bytes(enabled_byte)


soc = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
soc.bind(("0.0.0.0", 3333))

msg =  Message("veth1", True)
print(msg.as_bytes())

soc.sendto(msg.as_bytes(), ("172.16.200.2", 1234))