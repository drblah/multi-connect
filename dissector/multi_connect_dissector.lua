local multiconnect = Proto.new("multiconnect", "Multi-connect")

-- local field_connid = ProtoField.uint64("multiconnect.connid", "Conn ID", base.HEX)
local field_msg = ProtoField.uint8("multiconnect.msg_type", "Msg type", base.HEX)
local field_seqnr = ProtoField.uint64("multiconnect.seqnr", "Seq Nr.", base.DEC)
local field_peer_id = ProtoField.uint16("multiconnect.peer_id", "Peer ID", base.DEC)
local field_msg_length = ProtoField.uint64("multiconnect.msg_len", "Msg Len", base.DEC)
local field_tunnel_ip = ProtoField.ipv4("multiconnect.tun_ip", "Tunnel IP")
multiconnect.fields = { field_msg, field_seqnr, field_peer_id, field_msg_length, field_tunnel_ip }

-- Reverse of zigzag: https://docs.rs/bincode/latest/bincode/config/struct.VarintEncoding.html
function unzigzag(buffer)
    -- Decode u < 251
    if buffer(0, 1):le_uint() < 251 then
        return { 1, buffer(0, 1) }

        -- Decode 251 <= u < 2**16
    elseif buffer(0, 1):le_uint() == 251 then
        return { 3, buffer(1, 2) }

    -- Decode 2**16 <= u < 2**32
    elseif buffer(0, 1):le_uint() == 252 then
        return { 5, buffer(1, 4) }

    -- Decode 2**32 <= u < 2**64
        elseif buffer(0, 1):le_uint() == 253 then
        return { 9, buffer(1, 8) }
    end

end

-- the `dissector()` method is called by Wireshark when parsing our packets
-- `buffer` holds the UDP payload, all the bytes from our protocol
-- `tree` is the structure we see when inspecting/dissecting one particular packet
function multiconnect.dissector(buffer, pinfo, tree)
    -- Changing the value in the protocol column (the Wireshark pane that displays a list of packets) 
    pinfo.cols.protocol = "multi-connect"

    -- We label the entire UDP payload as being associated with our protocol
    local payload_tree = tree:add(multiconnect, buffer())

    local message_type_pos = 0
    local message_type_len = 4
    local message_type_buffer = buffer(message_type_pos, message_type_len)
    local message_type = message_type_buffer:le_uint()
    payload_tree:add_le(field_msg, message_type_buffer)

    if message_type == 0 then
        local seqnr_pos = message_type_pos + message_type_len
        local seqnr_len = 8
        payload_tree:add_le(field_seqnr, buffer(seqnr_pos, seqnr_len))

        local peer_id_pos = seqnr_pos + seqnr_len
        local peer_id_len = 2
        payload_tree:add_le(field_peer_id, buffer(peer_id_pos, peer_id_len))

        local msg_length_pos = peer_id_pos + peer_id_len
        local msg_length_len = 8
        payload_tree:add_le(field_msg_length, buffer(msg_length_pos, msg_length_maxlen))

        local payload_pos = msg_length_pos + msg_length_len
        local payload_len = buffer:len() - payload_pos
        local payload_buffer = buffer(payload_pos, payload_len)

        --Dissector.get("eth_withoutfcs"):call(ip_buffer:tvb(), pinfo, tree)
        -- Try to dissect as IP, else fall back to Ethernet
        local ipv4_byte = buffer(payload_pos, 1):le_uint()
        if ipv4_byte == 0x45 then
            Dissector.get("ip"):call(payload_buffer:tvb(), pinfo, tree)
        else
            Dissector.get("eth_withoutfcs"):call(payload_buffer:tvb(), pinfo, tree)
        end
    end
end

--we register our protocol on UDP port 51820
udp_table = DissectorTable.get("udp.port"):add(51820, multiconnect)
