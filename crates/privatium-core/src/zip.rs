// This file is part of Privatium
// crates/privatium-core/src/zip.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-10-08
// Last Modified: 2026-10-08
// Summary: A stored-only zip writer (PKWARE APPNOTE 6.3.x): local file headers, a central
//          directory, and the end-of-central-directory record. Method 0, no data descriptors, no
//          zip64, so the format is the 1989 one every extractor reads.
// Notes: See README file for documentation and full license information.
//
// Copyright © 2026 Gabriel Mongefranco
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along
// with this program. If not, see <https://www.gnu.org/licenses/>.

/// One entry's fixed DOS date-time: 2026-01-01 00:00:00. Reproducible output matters
/// more than a real mtime, and the files have none the binary could know.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1;

/// Write `entries` as `(name, bytes)`, in the order given.
#[must_use]
pub fn stored(entries: &[(String, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let name = name.as_bytes();
        let crc = crc32(data);
        let offset = u32::try_from(out.len()).unwrap_or(u32::MAX);
        let size = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let name_len = u16::try_from(name.len()).unwrap_or(u16::MAX);

        // Local file header.
        put32(&mut out, 0x0403_4b50);
        put16(&mut out, 20); // version needed: 2.0
        put16(&mut out, 0x0800); // flags: UTF-8 names
        put16(&mut out, 0); // method: stored
        put16(&mut out, DOS_TIME);
        put16(&mut out, DOS_DATE);
        put32(&mut out, crc);
        put32(&mut out, size);
        put32(&mut out, size);
        put16(&mut out, name_len);
        put16(&mut out, 0); // extra
        out.extend_from_slice(name);
        out.extend_from_slice(data);

        // Central directory entry.
        put32(&mut central, 0x0201_4b50);
        put16(&mut central, 20); // version made by
        put16(&mut central, 20); // version needed
        put16(&mut central, 0x0800);
        put16(&mut central, 0);
        put16(&mut central, DOS_TIME);
        put16(&mut central, DOS_DATE);
        put32(&mut central, crc);
        put32(&mut central, size);
        put32(&mut central, size);
        put16(&mut central, name_len);
        put16(&mut central, 0); // extra
        put16(&mut central, 0); // comment
        put16(&mut central, 0); // disk
        put16(&mut central, 0); // internal attributes
        put32(&mut central, 0); // external attributes
        put32(&mut central, offset);
        central.extend_from_slice(name);
    }

    let central_offset = u32::try_from(out.len()).unwrap_or(u32::MAX);
    let central_size = u32::try_from(central.len()).unwrap_or(u32::MAX);
    let count = u16::try_from(entries.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&central);

    // End of central directory.
    put32(&mut out, 0x0605_4b50);
    put16(&mut out, 0); // this disk
    put16(&mut out, 0); // central directory disk
    put16(&mut out, count);
    put16(&mut out, count);
    put32(&mut out, central_size);
    put32(&mut out, central_offset);
    put16(&mut out, 0); // comment
    out
}

/// CRC-32 (IEEE 802.3, reflected, polynomial `0xEDB88320`), as zip requires.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
