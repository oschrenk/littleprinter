//! Payload encoder for the 2012 BERG Little Printer.
//!
//! Byte layout per LAB-150 comment #144, read from `nordprojects/sirius` at
//! HEAD 9d88c63: `sirius/coding/encoders.py` and `sirius/coding/image_encoding.py`.
//!
//! Pixels are `bool`, true = white, row-major, 384 px wide.

/// Chunk sizes and their codes for runs longer than 251.
const TRANSLATE: [(u32, u8); 5] = [(1536, 255), (1152, 254), (768, 253), (384, 252), (251, 251)];

const CONTROL: [u8; 13] = [
    0x1d, 0x73, 0x03, 0xe8, // GS s  max printer speed
    0x1d, 0x61, 0xd0, // GS a  acceleration
    0x1d, 0x2f, 0x0f, // GS /  peak current
    0x1d, 0x44, 0x80, // GS D  max intensity
];

pub const WIDTH: u32 = 384;
pub const MAX_HEIGHT: u32 = 10000;
const LITTLE_PRINTER_DEVICE_ID: u8 = 1;

/// Append one run length in the printer's RLE scheme.
///
/// Runs of 0..=251 are the length itself. Longer runs are split into chunks
/// from `TRANSLATE`; every chunk after the first is preceded by a zero-length
/// run of the opposite colour so the decoder snaps back.
fn push_run(out: &mut Vec<u8>, length: u32) {
    if length <= 251 {
        out.push(length as u8);
        return;
    }
    let mut remainder = length;
    let mut done_first = false;
    while remainder > 251 {
        // remainder > 251 guarantees the (251, 251) entry matches.
        let (chunk, code) = TRANSLATE
            .iter()
            .copied()
            .find(|t| remainder >= t.0)
            .expect("TRANSLATE covers every remainder above 251");
        remainder -= chunk;
        if done_first {
            out.push(0);
        } else {
            done_first = true;
        }
        out.push(code);
    }
    if remainder > 0 {
        out.push(0);
        out.push(remainder as u8); // remainder <= 251 here
    }
}

/// Rotate 180 degrees, group into runs, RLE, prefix type and length.
///
/// Returns (pixel count, encoded block).
pub fn rle_from_bw(white: &[bool]) -> (usize, Vec<u8>) {
    let mut lengths: Vec<u32> = Vec::new();

    // ROTATE_180 on a row-major bitmap is the same as reading it backwards.
    let mut px = white.iter().rev().copied();
    let first = match px.next() {
        Some(p) => p,
        None => return (0, vec![0x01, 0, 0, 0, 0]),
    };

    // Sirius means to force a leading white run here, but compares an int from
    // getdata() against the str '\x00', so on Python 3 the guard never fires and
    // a black first pixel inverts the whole print. We emit the run it meant to.
    // Deliberate divergence from Python: see the diverges_from_python test.
    if !first {
        lengths.push(0);
    }

    let mut cur = first;
    let mut run: u32 = 1;
    for p in px {
        if p == cur {
            run += 1;
        } else {
            lengths.push(run);
            cur = p;
            run = 1;
        }
    }
    lengths.push(run);

    let mut data = Vec::new();
    for l in lengths {
        push_run(&mut data, l);
    }

    let mut out = Vec::with_capacity(5 + data.len());
    out.push(0x01); // compression type, always 1
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    (white.len(), out)
}

/// Wrap an RLE block in its 36-byte pixel-block header.
pub fn encode_pixels(pixel_count: usize, rle_image: &[u8]) -> Result<Vec<u8>, String> {
    let n = pixel_count / 8;
    if n > 0xff_ffff {
        // The printer's ESC * byte count is three bytes; Python's struct.pack
        // would raise here rather than truncate.
        return Err(format!(
            "{pixel_count} pixels exceeds the 3-byte ESC * byte count"
        ));
    }
    let printer_data: [u8; 8] = [
        0x1b,
        0x2a,
        (n & 0xff) as u8,
        ((n >> 8) & 0xff) as u8,
        ((n >> 16) & 0xff) as u8,
        0,
        0,
        48,
    ];

    let region_len = (CONTROL.len() + printer_data.len()) as u32; // 21
    let header_region_len = 5 + region_len as usize; // 26

    let mut out = Vec::with_capacity(5 + header_region_len + rle_image.len());
    out.extend_from_slice(&((header_region_len + rle_image.len() + 1) as u32).to_le_bytes());
    out.push(0);
    out.push(0);
    out.extend_from_slice(&region_len.to_le_bytes());
    out.extend_from_slice(&CONTROL);
    out.extend_from_slice(&printer_data);
    out.extend_from_slice(rle_image);
    Ok(out)
}

/// Build one pixel block straight from a bitmap.
pub fn payload_from_pixels(white: &[bool]) -> Result<Vec<u8>, String> {
    let (count, rle) = rle_from_bw(white);
    encode_pixels(count, &rle)
}

/// Wrap a payload in the 16-byte outer printer-message header.
pub fn encode_printer_message(command: u16, payload: &[u8], print_id: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + payload.len());
    out.push(LITTLE_PRINTER_DEVICE_ID);
    out.push(0); // reserved
    out.extend_from_slice(&command.to_le_bytes());
    out.extend_from_slice(&print_id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // CRC, unimplemented in Sirius
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// The commands that carry pixel blocks, with their opcode and image count.
pub const COMMANDS: [(&str, u16, usize); 7] = [
    ("set-delivery-and-print", 0x1, 1),
    ("set-delivery", 0x2, 1),
    ("set-delivery-and-print-no-face", 0x11, 1),
    ("set-delivery-no-face", 0x12, 1),
    ("set-personality-with-message", 0x0101, 5),
    ("set-personality", 0x0102, 4),
    ("set-quip", 0x202, 3),
];

/// Encode a complete command from one bitmap per image slot.
pub fn encode_command(
    command: u16,
    print_id: u32,
    images: &[Vec<bool>],
) -> Result<Vec<u8>, String> {
    let mut payload = Vec::new();
    for im in images {
        payload.extend_from_slice(&payload_from_pixels(im)?);
    }
    Ok(encode_printer_message(command, &payload, print_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(path: &str) -> Vec<bool> {
        crate::png_in::load_bw(std::path::Path::new(path))
            .unwrap()
            .0
    }

    fn fixture(name: &str) -> Vec<bool> {
        bitmap(&format!(
            "{}/testdata/{name}.png",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    /// Hand-derived in LAB-154, and independent of Python working at all.
    #[test]
    fn white_row_is_the_37_bytes_from_the_ticket() {
        let expected: [u8; 37] = [
            0x21, 0x00, 0x00, 0x00, 0x00, // u32 L=33, then 0x00
            0x00, 0x15, 0x00, 0x00, 0x00, // 0x00, then u32 21
            0x1d, 0x73, 0x03, 0xe8, 0x1d, 0x61, 0xd0, 0x1d, 0x2f, 0x0f, 0x1d, 0x44, 0x80, 0x1b,
            0x2a, 0x30, 0x00, 0x00, 0x00, 0x00, 0x30, // ESC *, n1=48
            0x01, 0x01, 0x00, 0x00, 0x00, 0xfc, // RLE type 1, len 1, data 0xfc
        ];
        let got = payload_from_pixels(&vec![true; 384]).unwrap();
        assert_eq!(got, expected, "got {}", hex(&got));
    }

    /// Runs longer than 251 chunk, and every chunk after the first takes a
    /// leading zero. 1920 = 1536 + 384, so: code 255, snap-back 0, code 252.
    #[test]
    fn long_run_chunks_with_snap_back_zeros() {
        let (count, rle) = rle_from_bw(&vec![true; 1920]);
        assert_eq!(count, 1920);
        assert_eq!(
            rle,
            vec![0x01, 0x03, 0, 0, 0, 0xff, 0x00, 0xfc],
            "{}",
            hex(&rle)
        );
    }

    /// Every fixture whose first post-rotation pixel is white must agree with
    /// Python byte for byte.
    #[test]
    fn golden_vectors_match_python_byte_for_byte() {
        let cases: [(&str, u16, &[&str]); 2] = [
            ("sdap_white1", 0x1, &["white1"]),
            ("sdap_barcodehead", 0x1, &["barcodehead"]),
        ];
        for (name, command, names) in cases {
            let images: Vec<Vec<bool>> = names.iter().map(|n| fixture(n)).collect();
            let got = encode_command(command, 0x12345678, &images).unwrap();
            let want = golden(name);
            assert_eq!(got.len(), want.len(), "{name} length");
            assert_eq!(got, want, "{name}\n got {}\nwant {}", hex(&got), hex(&want));
        }
    }

    fn golden(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/testdata/{name}.bin",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Sirius's leading-white-run guard compares an int against a str, so it
    /// never fires on Python 3 and a black first pixel inverts the whole print.
    /// We emit the zero-length white run Sirius intended. That makes our output
    /// one byte longer and differ in three places. Do NOT "fix" this back into
    /// agreement with Python: agreement here means an inverted print.
    #[test]
    fn black_first_pixel_diverges_from_python() {
        let got = encode_command(0x1, 0x12345678, &[vec![false; 384]]).unwrap();
        let want = golden("sdap_black1");

        assert_eq!(got.len(), want.len() + 1, "we add the one missing run byte");
        // Python's all-black row encodes to exactly its all-white row: total inversion.
        assert_eq!(want, golden("sdap_white1"));

        // Payload length and block length each grow by one; the RLE data gains
        // the leading 0x00 white run before the 0xfc black run.
        assert_eq!(&got[12..16], &38u32.to_le_bytes()); // payload len 37 -> 38
        assert_eq!(&want[12..16], &37u32.to_le_bytes());
        assert_eq!(got[16], 0x22); // block L 33 -> 34
        assert_eq!(want[16], 0x21);
        assert_eq!(&got[48..54], &[0x02, 0x00, 0x00, 0x00, 0x00, 0xfc]);
        assert_eq!(&want[48..53], &[0x01, 0x00, 0x00, 0x00, 0xfc]);
        // Everything before the first changed byte is identical.
        assert_eq!(&got[..12], &want[..12]);
        assert_eq!(&got[17..48], &want[17..48]);
    }

    /// normalface.png's own bottom-right pixel is black, so the real BERG
    /// artwork hits the same bug. Sirius's output for it inverts.
    #[test]
    fn normalface_is_black_first_and_diverges_too() {
        let got = encode_command(0x1, 0x12345678, &[fixture("normalface")]).unwrap();
        let want = golden("sdap_normalface");
        assert_ne!(got, want);
        assert_eq!(got.len(), want.len() + 1);
    }

    /// SetPersonality concatenates four blocks in field order. normalface and
    /// black1 are black-first, so this diverges from Python by exactly two
    /// inserted bytes, one per affected block.
    #[test]
    fn set_personality_concatenates_four_blocks() {
        let images: Vec<Vec<bool>> = ["normalface", "barcodehead", "white1", "black1"]
            .iter()
            .map(|n| fixture(n))
            .collect();
        let got = encode_command(0x0102, 0x12345678, &images).unwrap();
        let want = golden("personality");
        assert_eq!(got.len(), want.len() + 2);
        assert_eq!(&got[..4], &[0x01, 0x00, 0x02, 0x01]); // opcode 0x0102 LE
                                                          // The barcodehead and white1 blocks are white-first, so they are
                                                          // byte-identical to Python inside the concatenation.
        let bare = encode_command(0x1, 0x12345678, &[fixture("barcodehead")]).unwrap();
        let block = &bare[16..];
        assert!(
            got.windows(block.len()).any(|w| w == block),
            "the white-first barcodehead block must appear verbatim"
        );
    }

    /// A greyscale pixel would need Pillow's Floyd-Steinberg dither to match,
    /// which is out of scope, so it is refused rather than guessed at.
    #[test]
    fn grey_input_is_refused() {
        let p = format!("{}/testdata/grey.png", env!("CARGO_MANIFEST_DIR"));
        let e = crate::png_in::load_bw(std::path::Path::new(&p)).unwrap_err();
        assert!(e.contains("not pure black or white"), "{e}");
    }
}
