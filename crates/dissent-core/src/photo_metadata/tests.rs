use super::*;
use std::io::Cursor;

/// A real 16 x 8 picture in `format`, made by the image crate.
fn picture(format: image::ImageFormat) -> Vec<u8> {
    let img = image::RgbImage::from_fn(16, 8, |x, y| image::Rgb([x as u8 * 16, y as u8 * 32, 128]));
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, format).expect("encode");
    out.into_inner()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn segment(code: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = vec![0xFF, code];
    s.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    s.extend_from_slice(payload);
    s
}

/// An Exif block as a phone writes it: Orientation 6 (turn right) and a GPS directory, little-endian.
fn exif_with_gps() -> Vec<u8> {
    let mut tiff = b"II\x2A\0\x08\0\0\0".to_vec();
    tiff.extend_from_slice(&2u16.to_le_bytes());
    // Orientation = 6
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&6u16.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]);
    // GPS IFD pointer, to a directory that holds a latitude we can look for.
    tiff.extend_from_slice(&0x8825u16.to_le_bytes());
    tiff.extend_from_slice(&4u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&38u32.to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.extend_from_slice(b"GPS 37.3349 N 122.0090 W Make=PhoneCo");
    let mut payload = EXIF_ID.to_vec();
    payload.extend_from_slice(&tiff);
    payload
}

/// `jpeg` with Exif (GPS), XMP, IPTC, an MPF block, a comment and an ICC profile put after its JFIF header.
fn jpeg_with_metadata(jpeg: &[u8]) -> Vec<u8> {
    let app0_end = 2 + 2 + u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize;
    assert_eq!(jpeg[3], 0xE0, "the encoder writes JFIF first");
    let mut out = jpeg[..app0_end].to_vec();
    out.extend(segment(0xE1, &exif_with_gps()));
    out.extend(segment(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>exif:GPSLatitude 37.3349</x:xmpmeta>"));
    out.extend(segment(0xED, b"Photoshop 3.0\08BIM IPTC City=Cupertino"));
    out.extend(segment(0xE2, b"MPF\0second image with its own Exif"));
    out.extend(segment(0xE2, b"ICC_PROFILE\0\x01\x01sRGB profile bytes"));
    out.extend(segment(0xFE, b"taken at home"));
    out.extend_from_slice(&jpeg[app0_end..]);
    out
}

#[test]
fn a_jpeg_loses_its_location_camera_and_comments_but_keeps_pixels_colour_and_rotation() {
    let clean = picture(image::ImageFormat::Jpeg);
    let tagged = jpeg_with_metadata(&clean);
    assert!(contains(&tagged, b"37.3349"));
    let stripped = strip_image("image/jpeg", &tagged).expect("metadata to remove");

    for secret in [&b"37.3349"[..], b"PhoneCo", b"Cupertino", b"taken at home", b"second image", b"xmpmeta"] {
        assert!(!contains(&stripped, secret), "{} is gone", String::from_utf8_lossy(secret));
    }
    assert!(contains(&stripped, b"ICC_PROFILE\0"), "the colour profile stays");
    assert!(contains(&stripped, b"JFIF"), "the JFIF header stays");
    // The rotation survives as an Exif block with Orientation 6 alone.
    let exif_at = stripped.windows(EXIF_ID.len()).position(|w| w == EXIF_ID).expect("an Exif block for the rotation");
    assert_eq!(exif_orientation(&stripped[exif_at + EXIF_ID.len()..]), Some(6));
    // Nothing from the start of the scan on changed, so the pixels are the same bytes.
    let scan = |d: &[u8]| d.windows(2).position(|w| w == [0xFF, 0xDA]).expect("a scan");
    assert_eq!(&stripped[scan(&stripped)..], &tagged[scan(&tagged)..]);
    let img = image::load_from_memory(&stripped).expect("still a JPEG");
    assert_eq!((img.width(), img.height()), (16, 8));
}

#[test]
fn a_jpeg_with_nothing_to_remove_is_left_alone() {
    let clean = picture(image::ImageFormat::Jpeg);
    assert_eq!(strip_image("image/jpeg", &clean), None);
    // An upright photo (Orientation 1) gets no new Exif block.
    let mut upright = exif_with_gps();
    let at = EXIF_ID.len() + 8 + 2 + 8;
    upright[at] = 1;
    let app0_end = 2 + 2 + u16::from_be_bytes([clean[4], clean[5]]) as usize;
    let mut tagged = clean[..app0_end].to_vec();
    tagged.extend(segment(0xE1, &upright));
    tagged.extend_from_slice(&clean[app0_end..]);
    let stripped = strip_image("image/jpeg", &tagged).expect("the GPS block to remove");
    assert!(!contains(&stripped, EXIF_ID));
}

#[test]
fn a_broken_jpeg_is_left_alone() {
    assert_eq!(strip_image("image/jpeg", b"\xFF\xD8\xFF\xE1\x00"), None);
    assert_eq!(strip_image("image/jpeg", b"not a jpeg"), None);
}

/// A PNG chunk with a zero CRC: the stripper drops these, so the CRC is never read.
fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    c.extend_from_slice(&[0; 4]);
    c
}

#[test]
fn a_png_loses_its_text_exif_and_time_chunks() {
    let clean = picture(image::ImageFormat::Png);
    let iend = clean.len() - 12;
    let mut tagged = clean[..iend].to_vec();
    tagged.extend(chunk(b"tEXt", b"Comment\0GPS 37.3349"));
    tagged.extend(chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0exif:GPSLatitude"));
    tagged.extend(chunk(b"eXIf", &exif_with_gps()[EXIF_ID.len()..]));
    tagged.extend(chunk(b"tIME", &[7, 234, 10, 29, 12, 0, 0]));
    tagged.extend_from_slice(&clean[iend..]);
    let stripped = strip_image("image/png", &tagged).expect("metadata to remove");
    assert_eq!(stripped, clean, "exactly the original chunks are left");
    assert_eq!(strip_image("image/png", &clean), None);
}

/// A WebP file in the extended format: a VP8X header that flags Exif and XMP, the picture, then the two chunks.
fn webp_with_metadata() -> Vec<u8> {
    let simple = picture(image::ImageFormat::WebP);
    let image_chunk = &simple[12..];
    let mut body = b"WEBP".to_vec();
    body.extend_from_slice(b"VP8X");
    body.extend_from_slice(&10u32.to_le_bytes());
    body.extend_from_slice(&[0x0C, 0, 0, 0]); // Exif and XMP flags
    body.extend_from_slice(&[15, 0, 0, 7, 0, 0]); // canvas 16 x 8, stored minus one
    body.extend_from_slice(image_chunk);
    for (kind, data) in [(b"EXIF", &exif_with_gps()[EXIF_ID.len()..]), (b"XMP ", &b"exif:GPSLatitude 37.3349 "[..])] {
        body.extend_from_slice(kind);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
        if data.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

#[test]
fn a_webp_loses_its_exif_and_xmp_and_their_flags() {
    let tagged = webp_with_metadata();
    let stripped = strip_image("image/webp", &tagged).expect("metadata to remove");
    assert!(!contains(&stripped, b"37.3349") && !contains(&stripped, b"EXIF") && !contains(&stripped, b"XMP "));
    let vp8x = stripped.windows(4).position(|w| w == b"VP8X").expect("VP8X stays");
    assert_eq!(stripped[vp8x + 8] & 0x0C, 0, "the Exif and XMP flags are cleared");
    assert_eq!(u32::from_le_bytes(stripped[4..8].try_into().unwrap()) as usize, stripped.len() - 8, "the RIFF size is right");
    let img = image::load_from_memory(&stripped).expect("still a WebP");
    assert_eq!((img.width(), img.height()), (16, 8));
}

#[test]
fn formats_not_handled_here_are_left_alone() {
    assert_eq!(strip_image("image/heic", b"anything"), None);
    assert_eq!(strip_image("image/gif", b"GIF89a"), None);
}

// ---- MP4 ----

fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut b = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(payload);
    b
}

/// A phone video's container: an Android-style `©xyz` and the camera in `udta`, Apple-style keyed location and
/// model in `meta`, a title that must stay, and the media data after the `moov`.
fn video_with_location() -> Vec<u8> {
    let udta = [
        mp4_box(b"\xA9xyz", b"\x00\x13\x15\xc7+37.3349-122.0090/"),
        mp4_box(b"\xA9mak", b"\x00\x07\x15\xc7PhoneCo"),
        mp4_box(b"\xA9nam", b"\x00\x0a\x15\xc7Birthday 1"),
    ]
    .concat();
    let key = |name: &str| mp4_box(b"mdta", name.as_bytes());
    let keys_payload = [
        vec![0, 0, 0, 0],
        3u32.to_be_bytes().to_vec(),
        key("com.apple.quicktime.location.ISO6709"),
        key("com.apple.quicktime.title"),
        key("com.apple.quicktime.model"),
    ]
    .concat();
    let data_box = |text: &str| mp4_box(b"data", &[&[0, 0, 0, 1, 0, 0, 0, 0][..], text.as_bytes()].concat());
    let ilst = [
        mp4_box(&1u32.to_be_bytes(), &data_box("+37.3349-122.0090+020.000/")),
        mp4_box(&2u32.to_be_bytes(), &data_box("Birthday 2")),
        mp4_box(&3u32.to_be_bytes(), &data_box("Phone 15")),
    ]
    .concat();
    let meta = [mp4_box(b"hdlr", &[0; 25]), mp4_box(b"keys", &keys_payload), mp4_box(b"ilst", &ilst)].concat();
    let moov = mp4_box(b"moov", &[mp4_box(b"mvhd", &[0; 100]), mp4_box(b"udta", &udta), mp4_box(b"meta", &meta)].concat());
    [mp4_box(b"ftyp", b"qt  \0\0\0\0qt  "), moov, mp4_box(b"mdat", &[0x42; 64])].concat()
}

#[test]
fn a_video_loses_its_location_and_camera_in_place_and_keeps_its_title() {
    let original = video_with_location();
    let patches = mp4_patches(&mut Cursor::new(&original)).expect("reads");
    assert_eq!(patches.len(), 4, "©xyz, ©mak, the keyed location and the keyed model: {patches:?}");
    let mut patched = original.clone();
    for p in &patches {
        let at = p.offset as usize;
        assert_eq!(&patched[at..at + 4], &p.bytes[..4], "a patch keeps the box size");
        patched[at..at + p.bytes.len()].copy_from_slice(&p.bytes);
    }
    assert_eq!(patched.len(), original.len(), "no offset in the file moves");
    for secret in [&b"37.3349"[..], b"PhoneCo", b"Phone 15"] {
        assert!(!contains(&patched, secret), "{} is gone", String::from_utf8_lossy(secret));
    }
    assert!(contains(&patched, b"Birthday 1") && contains(&patched, b"Birthday 2"), "the titles stay");
    assert!(contains(&patched, &[0x42; 64]), "the media data is untouched");
    assert!(mp4_patches(&mut Cursor::new(&patched)).expect("reads").is_empty(), "nothing is left to remove");
}

#[test]
fn a_video_without_metadata_or_not_an_mp4_needs_no_patch() {
    let plain = [mp4_box(b"ftyp", b"isom\0\0\0\0isom"), mp4_box(b"moov", &mp4_box(b"mvhd", &[0; 100])), mp4_box(b"mdat", &[1; 8])].concat();
    assert!(mp4_patches(&mut Cursor::new(&plain)).expect("reads").is_empty());
    assert!(mp4_patches(&mut Cursor::new(b"\x1aE\xdf\xa3 a matroska file")).expect("reads").is_empty());
}
