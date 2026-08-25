use base64::Engine as _;
use data_encoding::BASE32;

use super::args::IdentityEncoding;

const BASE256_ALPHABET: &str = concat!(
    "abcdefghijklmnop",
    "qrstuvxyzæø01234",
    "ABCDEFGHIJKLMNOP",
    "QRSTUWXYZÆØ56789",
    "αβγδεζηθικλμνξπρ",
    "στφχψωΓΔΘΛΞΠΣΦΨΩ",
    "БДЖЗИЛПЦЧШЩЪЫЭЮЯ",
    "бджзилпцчшщъыэюя",
    "ԱԲԳԴԵԶԷԸԹԺԻԽԾԿՀՁ",
    "ՂՃՄՅՆՇՈՉՊՋՎՐՑՒՔՖ",
    "ᚠᚢᚦᚱᚹᚺᚾᛈᛇᛉᛊᛏᛒᛖᛗᛟ",
    "ｲｳｵｶｷｹｻｼｽｾﾀﾁﾃﾄﾅﾇ",
    "ﾈﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾗﾘﾙﾚﾜ",
    "𐑐𐑑𐑒𐑔𐑕𐑗𐑙𐑳𐑶𐑸𐑹𐑺𐑻𐑽𐑾𐑿",
    "᱑᱕᱘᱙ᱚᱝᱟᱣᱦᱨᱬᱭᱰᱳᱶᱷ",
    "𐌳𐌸𐌾𐐀𐐁𐐂𐐆𐐇𐐈𐐉𐐊𐐋𐐌𐐍𐐎𐐏",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityEncodingError {
    InvalidEncoding(IdentityEncoding),
    InvalidLength { expected: usize, found: usize },
}

pub fn encode(bytes: &[u8], encoding: IdentityEncoding) -> String {
    match encoding {
        IdentityEncoding::Hex => hex(bytes),
        IdentityEncoding::Base32 => BASE32.encode(bytes),
        IdentityEncoding::Base64 => base64::engine::general_purpose::URL_SAFE.encode(bytes),
        IdentityEncoding::Base256 => {
            let alphabet: Vec<_> = BASE256_ALPHABET.chars().collect();
            bytes
                .iter()
                .map(|byte| alphabet[usize::from(*byte)])
                .collect()
        }
    }
}

pub fn decode_identity(
    value: &str,
    encoding: Option<IdentityEncoding>,
    expected: usize,
) -> Result<Vec<u8>, IdentityEncodingError> {
    if let Some(encoding) = encoding {
        return decode_exact(value, encoding, expected);
    }
    for encoding in [
        IdentityEncoding::Hex,
        IdentityEncoding::Base32,
        IdentityEncoding::Base64,
    ] {
        if let Ok(decoded) = decode_exact(value, encoding, expected) {
            return Ok(decoded);
        }
    }
    Err(IdentityEncodingError::InvalidEncoding(
        IdentityEncoding::Hex,
    ))
}

fn decode_exact(
    value: &str,
    encoding: IdentityEncoding,
    expected: usize,
) -> Result<Vec<u8>, IdentityEncodingError> {
    let decoded = match encoding {
        IdentityEncoding::Hex => decode_hex(value),
        IdentityEncoding::Base32 => BASE32.decode(value.as_bytes()).ok(),
        IdentityEncoding::Base64 => base64::engine::general_purpose::URL_SAFE.decode(value).ok(),
        IdentityEncoding::Base256 => decode_base256(value),
    }
    .ok_or(IdentityEncodingError::InvalidEncoding(encoding))?;
    if decoded.len() != expected {
        return Err(IdentityEncodingError::InvalidLength {
            expected,
            found: decoded.len(),
        });
    }
    Ok(decoded)
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect()
}

fn decode_base256(value: &str) -> Option<Vec<u8>> {
    value
        .chars()
        .map(|character| {
            BASE256_ALPHABET
                .chars()
                .position(|candidate| candidate == character)
                .and_then(|index| u8::try_from(index).ok())
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl std::fmt::Display for IdentityEncodingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEncoding(encoding) => {
                write!(formatter, "input is not valid {encoding:?} identity data")
            }
            Self::InvalidLength { expected, found } => write!(
                formatter,
                "decoded identity holds {found} bytes, expected {expected}"
            ),
        }
    }
}

impl std::error::Error for IdentityEncodingError {}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC: [u8; 64] = [
        0x0f, 0xaa, 0x68, 0x4e, 0xd2, 0x88, 0x67, 0xb9, 0x7f, 0x4a, 0x6a, 0x2d, 0xee, 0x5d, 0xf8,
        0xce, 0x97, 0x4e, 0x76, 0xb7, 0x01, 0x8e, 0x3f, 0x22, 0xa1, 0xc4, 0xcf, 0x26, 0x78, 0x57,
        0x0f, 0x20, 0xd0, 0x4a, 0xb2, 0x32, 0x74, 0x2b, 0xb4, 0xab, 0x3a, 0x13, 0x68, 0xbd, 0x46,
        0x15, 0xe4, 0xe6, 0xd0, 0x22, 0x4a, 0xb7, 0x1a, 0x01, 0x6b, 0xaf, 0x85, 0x20, 0xa3, 0x32,
        0xc9, 0x77, 0x87, 0x37,
    ];

    #[test]
    fn stock_identity_encodings_match_rns_1_4_2() {
        assert_eq!(
            encode(&PUBLIC, IdentityEncoding::Base32),
            "B6VGQTWSRBT3S72KNIW64XPYZ2LU45VXAGHD6IVBYTHSM6CXB4QNASVSGJ2CXNFLHIJWRPKGCXSONUBCJK3RUALLV6CSBIZSZF3YONY="
        );
        assert_eq!(
            encode(&PUBLIC, IdentityEncoding::Base64),
            "D6poTtKIZ7l_Smot7l34zpdOdrcBjj8iocTPJnhXDyDQSrIydCu0qzoTaL1GFeTm0CJKtxoBa6-FIKMyyXeHNw=="
        );
        assert_eq!(
            encode(&PUBLIC, IdentityEncoding::Base256),
            "pᛊЧπ𐑒ԹЦｾяλЩNᱶΦ𐐈ﾚՉπпｼbՀ9CᚢﾎﾜGчΔpA𐑐λｵSиLｷᛏØtЧﾄηvᱚᱟ𐑐CλｼøbЪᛟԶAᚱSﾓцԸY"
        );
        for encoding in [
            IdentityEncoding::Hex,
            IdentityEncoding::Base32,
            IdentityEncoding::Base64,
            IdentityEncoding::Base256,
        ] {
            assert_eq!(
                decode_identity(&encode(&PUBLIC, encoding), Some(encoding), PUBLIC.len()),
                Ok(PUBLIC.to_vec())
            );
        }
    }
}
