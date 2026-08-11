//! Protobuf 编解码与 Connect-RPC 帧处理。
//!
//! 与 Windsurf 服务端线格式完全一致：varint / length-delimited 字段、
//! Connect-RPC 帧（1 字节 flags + 4 字节大端长度 + payload，支持 gzip）。

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Read, Write};

use crate::fastcontext::{FastContextError, FcResult};

/// 编码无符号 varint。
pub fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value > 0x7f {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    out.push((value & 0x7f) as u8);
    out
}

/// 从 `data[offset..]` 解码 varint，成功时推进 offset。
pub fn decode_varint(data: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    while *offset < data.len() {
        let b = data[*offset];
        *offset += 1;
        value |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    None
}

/// 手写 Protobuf 编码器（仅 varint / length-delimited 字段）。
#[derive(Default)]
pub struct ProtobufEncoder {
    bytes: Vec<u8>,
}

impl ProtobufEncoder {
    /// 写一个 varint 字段（wire type 0）。
    pub fn write_varint(&mut self, field: u64, value: u64) {
        self.bytes.extend(encode_varint((field << 3) | 0));
        self.bytes.extend(encode_varint(value));
    }

    /// 写一个字符串字段（wire type 2）。
    pub fn write_string(&mut self, field: u64, value: &str) {
        self.write_bytes(field, value.as_bytes());
    }

    /// 写一个字节字段（wire type 2）。
    pub fn write_bytes(&mut self, field: u64, value: &[u8]) {
        self.bytes.extend(encode_varint((field << 3) | 2));
        self.bytes.extend(encode_varint(value.len() as u64));
        self.bytes.extend(value);
    }

    /// 写一个嵌套消息字段（wire type 2）。
    pub fn write_message(&mut self, field: u64, sub: &ProtobufEncoder) {
        self.write_bytes(field, &sub.bytes);
    }

    /// 取出编码后的字节。
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// 从原始 protobuf 数据中提取 UTF-8 字符串（长度 > 5 且可打印占比 > 0.75）。
/// 递归解析嵌套消息（最深 3 层），匹配 Python/Node 原版的 `extract_strings`。
pub fn extract_strings(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    extract_strings_inner(data, 0, &mut out);
    out
}

fn extract_strings_inner(data: &[u8], depth: u8, out: &mut Vec<String>) {
    if depth > 3 {
        return;
    }
    let mut i = 0usize;
    while i < data.len() {
        let Some(tag) = decode_varint(data, &mut i) else {
            break;
        };
        match tag & 0x7 {
            0 => {
                let _ = decode_varint(data, &mut i);
            }
            1 => i = i.saturating_add(8).min(data.len()),
            2 => {
                let Some(length) = decode_varint(data, &mut i).map(|v| v as usize) else {
                    break;
                };
                if i + length > data.len() {
                    break;
                }
                let raw = &data[i..i + length];
                let text = String::from_utf8_lossy(raw).replace('\u{fffd}', "");
                if text.len() > 5 && printable_score(&text) > 0.75 {
                    out.push(text);
                }
                extract_strings_inner(raw, depth + 1, out);
                i += length;
            }
            5 => i = i.saturating_add(4).min(data.len()),
            _ => break,
        }
    }
}

/// 可打印字符占比（控制字符不算可打印，换行/回车/Tab 除外）。
fn printable_score(text: &str) -> f32 {
    let total = text.chars().count().max(1) as f32;
    let printable = text
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .count() as f32;
    printable / total
}

/// 把 protobuf 字节编码为 Connect-RPC 帧（可选 gzip 压缩 payload）。
pub fn connect_frame_encode(proto_bytes: &[u8], compress: bool) -> FcResult<Vec<u8>> {
    let (flags, payload) = if compress {
        (1u8, gzip_bytes(proto_bytes)?)
    } else {
        (0u8, proto_bytes.to_vec())
    };
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(flags);
    frame.extend((payload.len() as u32).to_be_bytes());
    frame.extend(payload);
    Ok(frame)
}

/// 解码 Connect-RPC 帧（支持 gzip 压缩帧，flags 1 或 3）。
pub fn connect_frame_decode(data: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut i = 0usize;
    while i + 5 <= data.len() {
        let flags = data[i];
        let length =
            u32::from_be_bytes([data[i + 1], data[i + 2], data[i + 3], data[i + 4]]) as usize;
        i += 5;
        if i + length > data.len() {
            break;
        }
        let payload = &data[i..i + length];
        i += length;
        if matches!(flags, 1 | 3) {
            frames.push(gunzip_bytes(payload).unwrap_or_else(|_| payload.to_vec()));
        } else {
            frames.push(payload.to_vec());
        }
    }
    frames
}

/// gzip 压缩原始字节。
pub fn gzip_bytes(data: &[u8]) -> FcResult<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| FastContextError::network(e.to_string()))?;
    encoder
        .finish()
        .map_err(|e| FastContextError::network(e.to_string()))
}

/// gzip 解压原始字节。
pub fn gunzip_bytes(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut decoder = GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

/// 把文本中无效 UTF-8 替换为空（等价 Python `errors="ignore"`），
/// 保留 `[TOOL_CALLS]` 提取所需的合法文本。
pub fn strip_invalid_utf8(data: &[u8]) -> String {
    String::from_utf8_lossy(data).replace('\u{fffd}', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protobuf_varint_round_trip() {
        for value in [0, 1, 127, 128, 16_384, u32::MAX as u64] {
            let bytes = encode_varint(value);
            let mut offset = 0;
            assert_eq!(decode_varint(&bytes, &mut offset), Some(value));
            assert_eq!(offset, bytes.len());
        }
    }

    #[test]
    fn connect_frame_round_trip_supports_gzip_and_plain() {
        let payload = b"hello fast-context";

        let compressed = connect_frame_encode(payload, true).expect("gzip 帧应可编码");
        assert_eq!(connect_frame_decode(&compressed), vec![payload.to_vec()]);

        let plain = connect_frame_encode(payload, false).expect("plain 帧应可编码");
        assert_eq!(connect_frame_decode(&plain), vec![payload.to_vec()]);
    }

    #[test]
    fn encoder_writes_expected_wire_format() {
        let mut encoder = ProtobufEncoder::default();
        encoder.write_varint(2, 5);
        encoder.write_string(3, "hello");
        let bytes = encoder.into_bytes();
        assert_eq!(bytes[0], (2 << 3) | 0); // field 2 varint tag
        assert_eq!(bytes[1], 5);
        assert_eq!(bytes[2], (3 << 3) | 2); // field 3 string tag
        assert_eq!(bytes[3], 5); // len
        assert_eq!(&bytes[4..], b"hello");
    }

    #[test]
    fn extract_strings_finds_utf8_and_skips_binary() {
        let mut encoder = ProtobufEncoder::default();
        encoder.write_string(1, "abcdef"); // len 6 > 5
        encoder.write_string(2, "abc"); // len 3，被过滤
        encoder.write_bytes(3, &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05]); // 控制字符，可打印占比低
        let strings = extract_strings(&encoder.into_bytes());
        assert_eq!(strings, vec!["abcdef"]);
    }

    #[test]
    fn strip_invalid_utf8_removes_replacement_chars() {
        let data = b"hello\xff\xfeworks";
        assert_eq!(strip_invalid_utf8(data), "helloworks");
    }
}
