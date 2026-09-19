//! 一个只会「存储」（不压缩）的最小 ZIP 写入器，给反馈页导出诊断包用。
//!
//! ## 为什么自己写
//!
//! 诊断包里就是几个纯文本文件，总共一两百 KB。为此拖进 `zip` + `flate2`
//! （再连坐一个 C 的 zlib 或者 miniz 的 Rust 实现）不划算 —— 启动器的卖点之一是包体小，
//! 而且每多一个依赖就多一处三平台交叉编译要验的东西。
//!
//! 存储法（method 0）是 ZIP 规范里最老、最普适的一档：`unzip`、Windows 资源管理器、
//! macOS 归档工具、Python `zipfile`、GitHub 的附件预览全都认。代价是文件不变小 ——
//! 对一个一两百 KB 的诊断包来说无所谓。
//!
//! 实现的是 APPNOTE 6.3.x 里最小的那一组结构：
//! 本地文件头（0x04034b50）→ 文件数据 → 中央目录（0x02014b50）→ 结尾记录（0x06054b50）。
//! 不用数据描述符（我们写之前就知道长度），不用 ZIP64（诊断包不可能上 4 GB）。

/// 一个待写入的条目。内容是 `Vec<u8>` —— 诊断包里全是已经脱敏过的文本。
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

/// 把若干条目打成一个 ZIP 的字节流。
///
/// `dos_time` / `dos_date` 传 0 即可（很多工具会显示成 1980-01-01）；
/// 我们在文件名和内容里都写了上海时间，不指望 ZIP 的时间戳。
pub fn write(entries: &[Entry]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    let mut count: u16 = 0;

    for e in entries {
        let name = sanitize_name(&e.name);
        let offset = out.len() as u32;
        let crc = crc32(&e.data);
        let size = e.data.len() as u32;

        // ── 本地文件头 ──
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes()); // 签名
        out.extend_from_slice(&20u16.to_le_bytes()); // 需要的版本 2.0
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // 通用标志：bit 11 = 文件名是 UTF-8
        out.extend_from_slice(&0u16.to_le_bytes()); // 压缩方法 0 = 存储
        out.extend_from_slice(&0u16.to_le_bytes()); // 修改时间
        out.extend_from_slice(&0u16.to_le_bytes()); // 修改日期
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes()); // 压缩后大小
        out.extend_from_slice(&size.to_le_bytes()); // 原始大小
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // 扩展字段长度
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&e.data);

        // ── 中央目录条目 ──
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // 制作版本
        central.extend_from_slice(&20u16.to_le_bytes()); // 需要的版本
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // 扩展字段
        central.extend_from_slice(&0u16.to_le_bytes()); // 注释
        central.extend_from_slice(&0u16.to_le_bytes()); // 磁盘号
        central.extend_from_slice(&0u16.to_le_bytes()); // 内部属性
        central.extend_from_slice(&0o100_644u32.wrapping_shl(16).to_le_bytes()); // 外部属性：普通文件 644
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());

        count = count.saturating_add(1);
    }

    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);

    // ── 中央目录结尾记录 ──
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // 本磁盘号
    out.extend_from_slice(&0u16.to_le_bytes()); // 中央目录所在磁盘
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // 注释长度
    out
}

/// 条目名的清洗：去掉盘符、前导斜杠与 `..`，免得解压时跳出目录（zip slip）。
/// 诊断包的名字都是我们自己给的，这一层是防御性的。
fn sanitize_name(name: &str) -> String {
    let n = name.replace('\\', "/");
    let parts: Vec<&str> = n
        .split('/')
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .collect();
    if parts.is_empty() {
        "file".to_string()
    } else {
        parts.join("/")
    }
}

/// CRC-32（IEEE 802.3 多项式，ZIP 用的就是它）。查表版，表在首次使用时算出来。
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc = table[((crc ^ *b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_对得上标准测试向量() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn 结构签名与条目数正确() {
        let z = write(&[
            Entry {
                name: "a.txt".into(),
                data: b"hello".to_vec(),
            },
            Entry {
                name: "dir/b.txt".into(),
                data: "中文内容".as_bytes().to_vec(),
            },
        ]);
        assert_eq!(
            &z[0..4],
            &0x0403_4b50u32.to_le_bytes(),
            "开头必须是本地文件头签名"
        );
        // 结尾记录固定 22 字节（无注释）
        let eocd = &z[z.len() - 22..];
        assert_eq!(&eocd[0..4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(u16::from_le_bytes([eocd[8], eocd[9]]), 2, "条目数应当是 2");
        // 内容原样在里面（存储法不压缩）
        assert!(
            z.windows(5).any(|w| w == b"hello"),
            "存储法下内容应当能直接找到"
        );
    }

    #[test]
    fn 条目名会被清洗掉跳出目录的写法() {
        assert_eq!(sanitize_name("/etc/passwd"), "etc/passwd");
        assert_eq!(sanitize_name("../../x"), "x");
        assert_eq!(sanitize_name("a\\b"), "a/b");
        assert_eq!(sanitize_name("../.."), "file");
    }

    #[test]
    fn 空包也是合法的_zip() {
        let z = write(&[]);
        assert_eq!(z.len(), 22);
        assert_eq!(&z[0..4], &0x0605_4b50u32.to_le_bytes());
    }
}
