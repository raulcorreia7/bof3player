//! Mode 2 Form 1 sectors: the error detection code (EDC) and the parity that seal them.
//!
//! A Form 1 sector is 2352 raw bytes: a 12-byte sync, a 4-byte header (the address), an 8-byte
//! sub-header and 2048 user bytes, then the 4-byte EDC and 276 parity bytes. The EDC is a CRC-32 over
//! bytes 16 to 2072 (sub-header and user data). The parity is the ECMA-130 Annex B product code over
//! bytes 12 to 2248, with the address counted as zero: P covers 86 columns of 24 bytes, Q covers 52
//! columns of 43 bytes and includes P. [`seal`] writes both, and [`is_sealed`] checks them.
//!
//! Form 2 sectors (XA and STR streams) carry no parity, so [`is_form2`] tells them apart and their
//! callers leave them as they are. Mode 1 sectors are not handled.
//!
//! # Examples
//!
//! ```
//! let mut sector = [0u8; psx_formats::sector::RAW_SIZE];
//! psx_formats::sector::seal(&mut sector);
//! assert!(psx_formats::sector::is_sealed(&sector));
//! ```

/// Bytes in a raw CD sector.
///
/// # Examples
///
/// ```
/// assert_eq!(psx_formats::sector::RAW_SIZE, 2352);
/// ```
pub const RAW_SIZE: usize = 2352;
/// The mode byte, which is 2 for a Mode 2 sector.
const MODE_AT: usize = 15;
/// The sub-header's submode byte. Bit 0x20 marks a Form 2 sector.
const SUBMODE_AT: usize = 18;
const FORM2_BIT: u8 = 0x20;
/// Where the EDC covers from: the sub-header.
const EDC_FROM: usize = 16;
const EDC_AT: usize = 2072;
/// Where the parity code covers from: the address field.
const ECC_FROM: usize = 12;
const P_AT: usize = 2076;
const Q_AT: usize = 2248;
/// P parity: 86 columns of 2 bytes. Q parity: 52 columns of 2 bytes.
const P_BYTES: usize = 172;
const Q_BYTES: usize = 104;

/// The CRC-32 table of the EDC: reflected polynomial `0x8001801B`.
const EDC_TABLE: [u32; 256] = edc_table();

/// `FORWARD` multiplies by two in GF(256) (polynomial `0x11D`). `BACK` inverts the step that
/// combines the two parity bytes.
const GF: ([u8; 256], [u8; 256]) = gf_tables();

const fn edc_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 { (value >> 1) ^ 0xD801_8001 } else { value >> 1 };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const fn gf_tables() -> ([u8; 256], [u8; 256]) {
    let mut forward = [0_u8; 256];
    let mut back = [0_u8; 256];
    let mut index = 0;
    while index < 256 {
        let doubled = ((index << 1) ^ if index & 0x80 != 0 { 0x11D } else { 0 }) as u8;
        forward[index] = doubled;
        back[(index as u8 ^ doubled) as usize] = index as u8;
        index += 1;
    }
    (forward, back)
}

/// Whether a raw sector is Mode 2 Form 2: an XA or STR stream sector, which has no parity.
///
/// # Examples
///
/// ```
/// let mut sector = [0u8; psx_formats::sector::RAW_SIZE];
/// sector[15] = 2;
/// assert!(!psx_formats::sector::is_form2(&sector));
/// sector[18] = 0x20;
/// assert!(psx_formats::sector::is_form2(&sector));
/// ```
pub fn is_form2(sector: &[u8; RAW_SIZE]) -> bool {
    sector[MODE_AT] == 2 && sector[SUBMODE_AT] & FORM2_BIT != 0
}

/// The EDC of a Form 1 sector: its sub-header and user data.
///
/// # Examples
///
/// ```
/// let mut sector = [0u8; psx_formats::sector::RAW_SIZE];
/// assert_eq!(psx_formats::sector::edc(&sector), 0);
/// sector[16] = 1;
/// assert_ne!(psx_formats::sector::edc(&sector), 0);
/// ```
pub fn edc(sector: &[u8; RAW_SIZE]) -> u32 {
    sector[EDC_FROM..EDC_AT]
        .iter()
        .fold(0, |edc, &byte| (edc >> 8) ^ EDC_TABLE[((edc ^ u32::from(byte)) & 0xFF) as usize])
}

/// Writes the EDC and the parity of a Form 1 sector in place. Every byte before the EDC (the sync,
/// the address, the sub-header and the user data) is the input.
///
/// # Examples
///
/// ```
/// let mut sector = [0u8; psx_formats::sector::RAW_SIZE];
/// sector[15] = 2;
/// psx_formats::sector::seal(&mut sector);
/// assert!(psx_formats::sector::is_sealed(&sector));
/// ```
pub fn seal(sector: &mut [u8; RAW_SIZE]) {
    let edc = edc(sector).to_le_bytes();
    sector[EDC_AT..EDC_AT + 4].copy_from_slice(&edc);
    let mut p = [0_u8; P_BYTES];
    parity(sector, 86, 24, 2, 86, &mut p);
    sector[P_AT..Q_AT].copy_from_slice(&p);
    let mut q = [0_u8; Q_BYTES];
    parity(sector, 52, 43, 86, 88, &mut q);
    sector[Q_AT..].copy_from_slice(&q);
}

/// Whether a Form 1 sector's stored EDC and parity match its contents.
///
/// # Examples
///
/// ```
/// let mut sector = [0u8; psx_formats::sector::RAW_SIZE];
/// psx_formats::sector::seal(&mut sector);
/// sector[16] = 1;
/// assert!(!psx_formats::sector::is_sealed(&sector));
/// ```
pub fn is_sealed(sector: &[u8; RAW_SIZE]) -> bool {
    let mut sealed = *sector;
    seal(&mut sealed);
    sealed == *sector
}

/// One parity code: `majors` columns of `minors` bytes. Column `major` starts at byte
/// `(major / 2) * multiplier + major % 2` and steps by `step` (wrapping at the code's length). The
/// output holds the two parity bytes of each column, first the `a` byte of all columns, then `a ^ b`.
fn parity(sector: &[u8; RAW_SIZE], majors: usize, minors: usize, multiplier: usize, step: usize, out: &mut [u8]) {
    let (forward, back) = &GF;
    let length = majors * minors;
    for major in 0..majors {
        let mut index = (major >> 1) * multiplier + (major & 1);
        let mut a = 0_u8;
        let mut b = 0_u8;
        for _ in 0..minors {
            // The address field counts as zero.
            let byte = if index < 4 { 0 } else { sector[ECC_FROM + index] };
            index += step;
            if index >= length {
                index -= length;
            }
            a ^= byte;
            b ^= byte;
            a = forward[usize::from(a)];
        }
        let a = back[usize::from(forward[usize::from(a)] ^ b)];
        out[major] = a;
        out[majors + major] = a ^ b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_sector_verifies_and_one_changed_byte_does_not() {
        let mut sector = [0_u8; RAW_SIZE];
        sector[1..11].fill(0xFF);
        sector[MODE_AT] = 2;
        sector[SUBMODE_AT] = 0x08;
        for (index, byte) in sector[24..2072].iter_mut().enumerate() {
            *byte = (index as u32).wrapping_mul(2_654_435_761).to_le_bytes()[3];
        }
        assert!(!is_form2(&sector));
        seal(&mut sector);
        assert!(is_sealed(&sector));
        sector[100] ^= 1;
        assert!(!is_sealed(&sector));
    }
}
