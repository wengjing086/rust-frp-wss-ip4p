"""极简 STUN 服务器（Binding Request + OTHER-ADDRESS）。

用途：官方 frpc 的 xtcp 在打洞前必须先 ``nathole.Prepare([stun])``：

1. 向 STUN 要自己的映射地址；
2. 如果响应里带 OTHER-ADDRESS，就再向那个备用地址要一次；
3. 拿到 **两个** 地址才能做 NAT 行为分类 —— 少于两个直接报
   ``discover error: not enough addresses``（见 pkg/nathole/classify.go:44）。

所以只回一个 XOR-MAPPED-ADDRESS 是不够的，必须开第二个 socket 当备用地址。
离线测试机连不上 stun.easyvoip.com，没有这个服务 xtcp 会一直失败，
看起来像"兼容性坏了"，其实只是环境问题。

源码里不使用反斜杠十六进制转义，统一用 struct 生成字节，
避免编辑器/传输过程把转义变成真正的控制字符。
"""
import socket
import struct
import threading

MAGIC = struct.pack("!I", 0x2112A442)
ATTR_MAPPED = 0x0001
ATTR_XOR_MAPPED = 0x0020
ATTR_OTHER_ADDRESS = 0x802C
BINDING_REQUEST = 0x0001
BINDING_SUCCESS = 0x0101
FAMILY_IPV4 = 0x0001


def _addr_attr(attr_type, ip, port, xor):
    if xor:
        port_field = port ^ 0x2112
        mask = struct.pack("!I", 0x2112A442)
        ip_field = bytes(a ^ b for a, b in zip(ip, mask))
    else:
        port_field = port
        ip_field = ip
    payload = struct.pack("!HH", FAMILY_IPV4, port_field) + ip_field
    return struct.pack("!HH", attr_type, len(payload)) + payload


class StunServer:
    """两个 UDP socket：主地址 + OTHER-ADDRESS 指向的备用地址。"""

    def __init__(self):
        self.socks = []
        for _ in range(2):
            s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            s.bind(("127.0.0.1", 0))
            self.socks.append(s)
        self.port = self.socks[0].getsockname()[1]
        self.alt_port = self.socks[1].getsockname()[1]
        self.end = False
        self.hits = 0

    def start(self):
        for idx, s in enumerate(self.socks):
            other = self.socks[1 - idx]
            t = threading.Thread(target=self._serve, args=(s, other), daemon=True)
            t.start()
        return self

    def _serve(self, sock, other):
        while not self.end:
            try:
                data, addr = sock.recvfrom(2048)
            except OSError:
                return
            if len(data) < 20 or data[4:8] != MAGIC:
                continue
            if struct.unpack("!H", data[:2])[0] != BINDING_REQUEST:
                continue
            self.hits += 1
            tid = data[8:20]
            ip = socket.inet_aton(addr[0])
            oip, oport = other.getsockname()[0], other.getsockname()[1]
            attr = _addr_attr(ATTR_XOR_MAPPED, ip, addr[1], True)
            attr += _addr_attr(ATTR_MAPPED, ip, addr[1], False)
            attr += _addr_attr(ATTR_OTHER_ADDRESS, socket.inet_aton(oip), oport, False)
            # length 字段只算属性长度，不含 20 字节头（magic + tid 在内）
            resp = struct.pack("!HH", BINDING_SUCCESS, len(attr)) + MAGIC + tid + attr
            try:
                sock.sendto(resp, addr)
            except OSError:
                pass
