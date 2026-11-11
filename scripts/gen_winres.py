#!/usr/bin/env python3
"""生成 Windows COFF ``.res`` 资源对象（VERSIONINFO + 应用清单）。

为什么不用 rc.exe / winres / winresource
---------------------------------------
本机（以及很多干净的 CI 环境）没有 Windows SDK 的 ``rc.exe``。这个脚本
用纯 Python 直接写出 COFF 对象文件，``link.exe`` 可以像普通 .obj 一样吃进去，
于是 Release 二进制也能带上完整的版本资源与应用清单 —— **不需要任何编译期
外部工具**。生成结果提交在 ``assets/winres/``，可用本脚本随时重生成。

为什么需要版本资源
------------------
Windows 安全软件对「无版本信息、无产品名、无清单、需求管理员权限」的
未知可执行文件会给出很高的启发式风险分。补齐这些元数据是降低误报最
直接、也最合规的手段（不是免杀、不是加壳、不是混淆，只是把文件说明白）。

用法：
    python scripts/gen_winres.py            # 生成 assets/winres/*.res
    python scripts/gen_winres.py --check    # 只校验已生成的 .res 结构
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "assets" / "winres"

VERSION = (0, 8, 0, 0)
COMPANY = "wengjing086"
PRODUCT = "Rust Frp (WSS + IP4P)"
COPYRIGHT = "Copyright 2026 wengjing086. Apache-2.0."

RT_VERSION = 16
RT_MANIFEST = 24
LANG_ID = 0x0409  # 英语（美国），版本资源里最通用的语言 ID

# COFF 常量
MACHINE_AMD64 = 0x8664
MACHINE_I386 = 0x014C
REL_AMD64_ADDR32NB = 0x0003
REL_I386_DIR32NB = 0x0007
SECT_CHARACTERISTICS = 0x40300040  # CNT_INITIALIZED_DATA | ALIGN_4BYTES | MEM_READ


def wstr(s: str) -> bytes:
    return s.encode("utf-16-le") + b"\x00\x00"


def pad4(b: bytes) -> bytes:
    return b + b"\x00" * ((-len(b)) % 4)


def node(key: str, value: bytes, value_length: int, wtype: int, children: bytes = b"") -> bytes:
    """构造一个 VS_VERSIONINFO / StringFileInfo / String / Var 通用节点。"""
    header = struct.pack("<HHH", 0, value_length, wtype) + wstr(key)
    header = pad4(header)
    body = header + value + children
    # wLength 不含尾部 padding：Windows 的 VerQueryValue 会自己按 4 字节对齐走子节点。
    body = struct.pack("<H", len(body)) + body[2:]
    return pad4(body)


def string_entry(key: str, value: str) -> bytes:
    v = wstr(value)
    return node(key, v, len(v) // 2, 1)


def version_info(binary_name: str, description: str) -> bytes:
    major, minor, patch, build = VERSION
    ms = (major << 16) | minor
    ls = (patch << 16) | build
    fixed = struct.pack(
        "<13I",
        0xFEEF04BD,  # dwSignature
        0x00010000,  # dwStrucVersion
        ms, ls,      # dwFileVersionMS / LS
        ms, ls,      # dwProductVersionMS / LS
        0x0000003F,  # dwFileFlagsMask
        0x00000000,  # dwFileFlags (release, no debug/private build)
        0x00040004,  # dwFileOS = VOS_NT_WINDOWS32
        0x00000001,  # dwFileType = VFT_APP
        0, 0, 0,     # dwFileSubtype / dwFileDateMS / dwFileDateLS
    )
    strings = {
        "CompanyName": COMPANY,
        "FileDescription": description,
        "FileVersion": ".".join(map(str, VERSION)),
        "InternalName": binary_name,
        "LegalCopyright": COPYRIGHT,
        "OriginalFilename": binary_name,
        "ProductName": PRODUCT,
        "ProductVersion": ".".join(map(str, VERSION)),
        "Comments": "Open-source reverse proxy. Source: github.com/wengjing086/rust-frp-wss-ip4p",
    }
    table = b"".join(string_entry(k, v) for k, v in strings.items())
    string_file_info = node("StringFileInfo", b"", 0, 1, node("040904b0", b"", 0, 1, table))
    var = node("Translation", struct.pack("<HH", LANG_ID, 0x04B0), 4, 0)
    var_file_info = node("VarFileInfo", b"", 0, 1, var)
    # 根节点不 pad：rc.exe 的产物长度即 wLength
    header = struct.pack("<HHH", 0, len(fixed), 0) + wstr("VS_VERSION_INFO")
    header = pad4(header)
    body = header + fixed + string_file_info + var_file_info
    return struct.pack("<H", len(body)) + body[2:]


def manifest(binary_name: str) -> bytes:
    xml = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="{'.'.join(map(str, VERSION))}" processorArchitecture="*" name="{COMPANY}.{PRODUCT.replace(' ', '').replace('+', '')}.{binary_name}" type="win32"/>
  <description>{PRODUCT} - open source frp implementation in Rust</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{35138b9a-5d96-4fbd-8e2d-a2440225f93a}}"/>
      <supportedOS Id="{{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}}"/>
      <supportedOS Id="{{1f676c76-80e1-4239-95bb-83d0f6d0da78}}"/>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"""
    return xml.encode("utf-8")


def build_res(binary_name: str, description: str, machine: int) -> bytes:
    blobs = [
        (RT_VERSION, 1, LANG_ID, version_info(binary_name, description)),
        (RT_MANIFEST, 1, LANG_ID, manifest(binary_name)),
    ]

    # --- section 2：数据区，每个 blob 4 字节对齐 ---
    data = bytearray()
    data_offsets = []
    for _rtype, _name, _lang, blob in blobs:
        data_offsets.append(len(data))
        data += blob
        data += b"\x00" * ((-len(data)) % 4)
    sec2 = bytes(data)

    # --- section 1：资源目录树 ---
    directory = bytearray()
    root_entries = struct.pack("<HH", 0, 0) + struct.pack("<HH", 0, 0)  # 16B 头占位
    # 头部：Characteristics, TimeDateStamp, Major/Minor, Named/Id 数量
    root_header = struct.pack("<IIHHHH", 0, 0, 0, 0, 0, len(blobs))
    directory += root_header + b"\x00" * (len(blobs) * 8)  # 根条目占位
    relocation_offsets: list[int] = []
    child_off = 16 + len(blobs) * 8

    for i, (rtype, name_id, lang_id, _blob) in enumerate(blobs):
        type_dir_off = child_off
        name_dir_off = type_dir_off + 24
        data_entry_off = name_dir_off + 24
        next_off = data_entry_off + 16
        child_off = next_off

        # 根条目 -> 类型子目录
        struct.pack_into("<II", directory, 16 + i * 8, rtype, 0x80000000 | type_dir_off)
        # 类型目录 + 条目 -> 名称子目录
        directory += struct.pack("<IIHHHH", 0, 0, 0, 0, 0, 1)
        directory += struct.pack("<II", name_id, 0x80000000 | name_dir_off)
        # 名称目录 + 条目 -> 数据条目
        directory += struct.pack("<IIHHHH", 0, 0, 0, 0, 0, 1)
        directory += struct.pack("<II", lang_id, data_entry_off)
        # 数据条目：OffsetToData 需要一条 32 位 RVA 重定位
        directory += struct.pack("<IIII", data_offsets[i], len(_blob), 0, 0)
        relocation_offsets.append(data_entry_off)

    assert len(directory) == child_off, (len(directory), child_off)
    sec1 = bytes(directory)
    del root_entries

    # --- COFF 文件头 / 节头 ---
    n_relocs = len(relocation_offsets)
    header_size = 20 + 2 * 40
    sec1_off = header_size
    sec2_off = sec1_off + len(sec1)
    reloc_off = sec2_off + len(sec2)
    reloc_off += (-reloc_off) % 4
    symtab_off = reloc_off + n_relocs * 10
    string_table_off = symtab_off + 4 * 18

    reloc_type = REL_AMD64_ADDR32NB if machine == MACHINE_AMD64 else REL_I386_DIR32NB
    relocs = b"".join(
        struct.pack("<IIH", off, 2, reloc_type)  # 符号 2 = .rsrc$02 节符号
        for off in relocation_offsets
    )

    def section(name: bytes, size: int, offset: int, rel_ptr: int, rel_count: int) -> bytes:
        assert len(name) == 8
        return struct.pack(
            "<8sIIIIIIHHI",
            name, 0, 0, size, offset, rel_ptr, 0, rel_count, 0, SECT_CHARACTERISTICS,
        )

    file_header = struct.pack(
        "<HHIIIHH",
        machine, 2, 0, symtab_off, 4, 0, 0,
    )
    sec_headers = (
        section(b".rsrc$01", len(sec1), sec1_off, reloc_off, n_relocs)
        + section(b".rsrc$02", len(sec2), sec2_off, 0, 0)
    )

    def section_symbol(name: bytes, sect_no: int) -> bytes:
        return struct.pack("<8sIhHBB", name, 0, sect_no, 0, 3, 1)

    def aux_section(length: int, rel_count: int) -> bytes:
        return struct.pack("<IHHIHB3s", length, rel_count, 0, 0, 0, 0, b"\x00" * 3)

    symtab = (
        section_symbol(b".rsrc$01", 1)
        + aux_section(len(sec1), n_relocs)
        + section_symbol(b".rsrc$02", 2)
        + aux_section(len(sec2), 0)
    )
    assert len(symtab) == 4 * 18, len(symtab)
    string_table = struct.pack("<I", 4)  # 空字符串表，长度字段本身 4 字节

    out = bytearray()
    out += file_header + sec_headers
    out += sec1
    out += sec2
    out += b"\x00" * ((-len(out)) % 4)
    assert len(out) == reloc_off, (len(out), reloc_off)
    out += relocs
    assert len(out) == symtab_off
    out += symtab
    assert len(out) == string_table_off
    out += string_table
    return bytes(out)


def check(path: Path) -> None:
    b = path.read_bytes()
    machine, nsects, _, symptr, nsyms, _, _ = struct.unpack_from("<HHIIIHH", b, 0)
    assert machine in (MACHINE_AMD64, MACHINE_I386), hex(machine)
    assert nsects == 2 and nsyms == 4
    assert b[20:28] == b".rsrc$01" and b[60:68] == b".rsrc$02"
    assert symptr + 4 * 18 + 4 == len(b), (symptr, len(b))
    print(f"OK  {path.name}: {len(b)} B, machine=0x{machine:04x}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()

    if args.check:
        for p in sorted(OUT_DIR.glob("*.res")):
            check(p)
        return 0

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    targets = [
        ("frps.exe", "frp server (Rust) - native WSS / IP4P compatible", "frps"),
        ("frpc.exe", "frp client (Rust) - IP4P address resolver", "frpc"),
    ]
    for filename, description, stem in targets:
        for machine, suffix in ((MACHINE_AMD64, "x86_64"), (MACHINE_I386, "i686")):
            data = build_res(filename, description, machine)
            out = OUT_DIR / f"{stem}_{suffix}.res"
            out.write_bytes(data)
            check(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
