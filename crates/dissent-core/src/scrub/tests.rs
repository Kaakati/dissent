//! Each fixture is a real or hand-built file of its format carrying identifying details the way cameras, phones and
//! editors write them. After scrubbing, none of `IDENTIFYING` may appear anywhere in the bytes, the way a tag dump
//! (exiftool and the like) would find them, and the picture must still be there.

use super::*;
use std::io::Cursor;

/// What the fixtures carry: a position, a place, a maker and model, serial numbers, a person, a document id, dates and
/// the software that wrote them. Nothing of it may survive.
const IDENTIFYING: [&str; 14] = [
    "37.3349",
    "122.0090",
    "Cupertino",
    "PhoneCo",
    "Phone 15",
    "SN12345",
    "LENS-777",
    "Maya Doe",
    "Maya's MacBook",
    "DOC-1234",
    "2026:10:29",
    "2026-10-29",
    "CameraApp 9.1",
    "John's camera",
];

fn leaks(bytes: &[u8]) -> Vec<&'static str> {
    IDENTIFYING.iter().copied().filter(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes())).collect()
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dissent-scrub-{tag}-{}-{:x}", std::process::id(), rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Scrubs `bytes` (as a file named `name`) and returns the result, checking first that the fixture does carry details.
fn scrubbed(name: &str, bytes: &[u8]) -> (Scrubbed, Vec<u8>) {
    assert!(!leaks(bytes).is_empty(), "the {name} fixture carries details to remove");
    let dir = scratch("case");
    let (src, out) = (dir.join(name), dir.join(format!("out-{name}")));
    std::fs::write(&src, bytes).unwrap();
    let how = scrub(&src, &out).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    let result = std::fs::read(&out).unwrap();
    assert_eq!(std::fs::read(&src).unwrap(), bytes, "the original is never changed");
    let _ = std::fs::remove_dir_all(&dir);
    (how, result)
}

fn picture(format: image::ImageFormat) -> Vec<u8> {
    let img = image::RgbImage::from_fn(16, 8, |x, y| image::Rgb([x as u8 * 16, y as u8 * 32, 128]));
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, format).expect("encode");
    out.into_inner()
}

fn decodes(bytes: &[u8]) -> (u32, u32) {
    let img = image::load_from_memory(bytes).expect("still a picture");
    (img.width(), img.height())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

// ---- fixture parts ----

/// A little-endian Exif TIFF block: Orientation 6, the camera, an owner, serials, dates and a GPS directory.
fn exif_block() -> Vec<u8> {
    let mut t = b"II\x2A\0\x08\0\0\0".to_vec();
    t.extend_from_slice(&1u16.to_le_bytes());
    t.extend_from_slice(&0x0112u16.to_le_bytes());
    t.extend_from_slice(&3u16.to_le_bytes());
    t.extend_from_slice(&1u32.to_le_bytes());
    t.extend_from_slice(&6u16.to_le_bytes());
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(b"Make=PhoneCo Model=Phone 15 BodySerialNumber=SN12345 LensSerialNumber=LENS-777 CameraOwnerName=Maya Doe DateTimeOriginal=2026:10:29 15:30:00 GPS 37.3349 N 122.0090 W Software=CameraApp 9.1");
    t
}

/// A minimal ICC profile whose description and copyright name a person and a device.
fn icc_profile() -> Vec<u8> {
    let desc = b"descMaya's MacBook Pro display";
    let cprt = b"textCopyright Maya Doe";
    let mut p = vec![0u8; 128];
    p[36..40].copy_from_slice(b"acsp");
    p[48..52].copy_from_slice(b"PhCo"); // device manufacturer
    p[24..36].copy_from_slice(&[7, 234, 0, 10, 0, 29, 0, 15, 0, 30, 0, 0]); // 2026-10-29 15:30 (binary, not a leak)
    p.extend_from_slice(&2u32.to_be_bytes());
    let table_end = 128 + 4 + 2 * 12;
    let entries = [(b"desc", table_end, desc.len()), (b"cprt", table_end + desc.len(), cprt.len())];
    for (sig, off, len) in entries {
        p.extend_from_slice(sig);
        p.extend_from_slice(&(off as u32).to_be_bytes());
        p.extend_from_slice(&(len as u32 + 4).to_be_bytes()); // with the 4 reserved bytes below counted in
    }
    // Each tag: type signature, 4 reserved bytes, then its text.
    let mut tag = |kind: &[u8], text: &[u8]| {
        p.extend_from_slice(&kind[..4]);
        p.extend_from_slice(&[0; 4]);
        p.extend_from_slice(&text[4..]);
    };
    tag(desc, desc);
    tag(cprt, cprt);
    let len = p.len() as u32;
    p[0..4].copy_from_slice(&len.to_be_bytes());
    p
}

fn segment(code: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = vec![0xFF, code];
    s.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    s.extend_from_slice(payload);
    s
}

// ---- JPEG ----

fn tagged_jpeg() -> Vec<u8> {
    let clean = picture(image::ImageFormat::Jpeg);
    let app0_end = 4 + u16::from_be_bytes([clean[4], clean[5]]) as usize;
    let mut j = clean[..app0_end].to_vec();
    j.extend(segment(0xE0, b"JFXX\0\x10thumbnail of Maya Doe"));
    j.extend(segment(0xE1, &[b"Exif\0\0".as_slice(), &exif_block()].concat()));
    j.extend(segment(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta xmpMM:DocumentID=\"DOC-1234\" xmp:CreatorTool=\"CameraApp 9.1\" dc:creator=\"Maya Doe\"/>"));
    j.extend(segment(0xED, b"Photoshop 3.0\08BIM By-line=Maya Doe City=Cupertino"));
    j.extend(segment(0xE2, b"MPF\0second image, Exif Make=PhoneCo"));
    j.extend(segment(0xE2, &[b"ICC_PROFILE\0\x01\x01".as_slice(), &icc_profile()].concat()));
    j.extend(segment(0xEB, b"JUMBF c2pa manifest John's camera"));
    j.extend(segment(0xFE, b"shot by Maya Doe on 2026-10-29"));
    j.extend_from_slice(&clean[app0_end..]);
    j
}

#[test]
fn a_jpeg_keeps_its_picture_colour_and_rotation_and_nothing_that_identifies() {
    let tagged = tagged_jpeg();
    let (how, out) = scrubbed("photo.jpg", &tagged);
    assert_eq!(how, Scrubbed::Cleaned);
    assert_eq!(leaks(&out), Vec::<&str>::new());
    assert!(contains(&out, b"ICC_PROFILE\0") && contains(&out, b"acsp"), "the colour profile stays");
    assert!(!contains(&out, b"JFXX") && !contains(&out, b"MPF\0") && !contains(&out, b"xmpmeta"), "previews and XMP go");
    let exif = out.windows(6).position(|w| w == b"Exif\0\0").expect("an Exif block for the rotation");
    assert_eq!(jpeg::exif_orientation(&out[exif + 6..]), Some(6));
    let scan = |d: &[u8]| d.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    assert_eq!(&out[scan(&out)..], &tagged[scan(&tagged)..], "the compressed picture is the same bytes");
    assert_eq!(decodes(&out), (16, 8));
}

// ---- PNG ----

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    c.extend_from_slice(&[0; 4]); // the CRC is never read for a dropped chunk
    c
}

#[test]
fn a_png_keeps_only_what_draws_it() {
    let clean = picture(image::ImageFormat::Png);
    let iend = clean.len() - 12;
    let tagged = [
        &clean[..iend],
        &chunk(b"tEXt", b"Author\0Maya Doe"),
        &chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0xmpMM:DocumentID DOC-1234 GPS 37.3349"),
        &chunk(b"zTXt", b"Software\0\0CameraApp 9.1"),
        &chunk(b"eXIf", &exif_block()),
        &chunk(b"tIME", &[7, 234, 10, 29, 15, 30, 0]),
        &chunk(b"caBX", b"c2pa John's camera"),
        &clean[iend..],
    ]
    .concat();
    let (_, out) = scrubbed("photo.png", &tagged);
    assert_eq!(out, clean, "exactly the drawing chunks are left");
}

// ---- WebP ----

fn riff_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = kind.to_vec();
    c.extend_from_slice(&(data.len() as u32).to_le_bytes());
    c.extend_from_slice(data);
    if data.len() % 2 == 1 {
        c.push(0);
    }
    c
}

#[test]
fn a_webp_loses_exif_xmp_and_profile_text() {
    let simple = picture(image::ImageFormat::WebP);
    let mut body = b"WEBP".to_vec();
    body.extend(riff_chunk(b"VP8X", &[0x2C, 0, 0, 0, 15, 0, 0, 7, 0, 0])); // ICC, Exif and XMP flags; 16 x 8
    body.extend(riff_chunk(b"ICCP", &icc_profile()));
    body.extend_from_slice(&simple[12..]);
    body.extend(riff_chunk(b"EXIF", &exif_block()));
    body.extend(riff_chunk(b"XMP ", b"xmpMM:DocumentID DOC-1234 dc:creator Maya Doe"));
    let tagged = [b"RIFF".as_slice(), &(body.len() as u32).to_le_bytes(), &body].concat();
    let (_, out) = scrubbed("photo.webp", &tagged);
    assert_eq!(leaks(&out), Vec::<&str>::new());
    let vp8x = out.windows(4).position(|w| w == b"VP8X").unwrap();
    assert_eq!(out[vp8x + 8] & 0x0C, 0, "the Exif and XMP flags are cleared");
    assert!(contains(&out, b"ICCP") && contains(&out, b"acsp"), "the colour profile stays");
    assert_eq!(u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize, out.len() - 8);
    assert_eq!(decodes(&out), (16, 8));
}

// ---- GIF ----

#[test]
fn a_gif_keeps_its_frames_and_loop_and_loses_comments_and_xmp() {
    let clean = picture(image::ImageFormat::Gif);
    // After the header, logical screen descriptor and global colour table, before the first block.
    let gct = if clean[10] & 0x80 != 0 { 3 * (1 << ((clean[10] & 7) + 1)) } else { 0 };
    let at = 13 + gct;
    let mut ext = vec![0x21, 0xFF, 11];
    ext.extend_from_slice(b"NETSCAPE2.0");
    ext.extend_from_slice(&[3, 1, 0, 0, 0]);
    let mut comment = vec![0x21, 0xFE];
    let text = b"made by Maya Doe with CameraApp 9.1";
    comment.push(text.len() as u8);
    comment.extend_from_slice(text);
    comment.push(0);
    let mut xmp = vec![0x21, 0xFF, 11];
    xmp.extend_from_slice(b"XMP DataXMP");
    let packet = b"xmpMM:DocumentID DOC-1234 GPS 37.3349";
    xmp.push(packet.len() as u8);
    xmp.extend_from_slice(packet);
    xmp.push(0);
    let tagged = [&clean[..at], &ext, &comment, &xmp, &clean[at..]].concat();
    let (_, out) = scrubbed("photo.gif", &tagged);
    assert_eq!(leaks(&out), Vec::<&str>::new());
    assert!(contains(&out, b"NETSCAPE2.0"), "the loop count stays");
    assert_eq!(decodes(&out), (16, 8));
}

// ---- TIFF ----

/// A 2 x 2 RGB TIFF, little-endian, with the camera, the owner and dates in IFD0, an Exif directory with serials and a
/// MakerNote, and a GPS directory.
fn tagged_tiff() -> Vec<u8> {
    let pixels: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 9, 9];
    let texts: [(u16, &[u8]); 6] = [
        (0x010F, b"PhoneCo\0"),
        (0x0110, b"Phone 15\0"),
        (0x0131, b"CameraApp 9.1\0"),
        (0x0132, b"2026:10:29 15:30:00\0"),
        (0x013B, b"Maya Doe\0"),
        (0x8298, b"(c) Maya Doe\0"),
    ];
    let exif_texts: [(u16, &[u8]); 4] = [
        (0x9003, b"2026:10:29 15:30:00\0"),
        (0xA431, b"SN12345\0"),
        (0xA430, b"Maya Doe\0"),
        (0x927C, b"PhoneCo internal LENS-777"),
    ];
    let gps_texts: [(u16, &[u8]); 1] = [(0x001C, b"Cupertino 37.3349 N 122.0090 W")];
    // Layout: header, pixels, then each directory followed by its values.
    let mut out = b"II\x2A\0".to_vec();
    out.extend_from_slice(&0u32.to_le_bytes()); // IFD0 offset, set below
    let pixels_at = out.len() as u32;
    out.extend_from_slice(&pixels);
    let ifd = |out: &mut Vec<u8>, shorts: &[(u16, u16, u32)], texts: &[(u16, &[u8])], pointers: &[(u16, u32)]| -> (u32, Vec<usize>) {
        let at = out.len() as u32;
        let count = shorts.len() + texts.len() + pointers.len();
        out.extend_from_slice(&(count as u16).to_le_bytes());
        let mut pointer_fields = Vec::new();
        let values_at = at + 2 + count as u32 * 12 + 4;
        let mut values = Vec::new();
        let mut all: Vec<(u16, Vec<u8>)> = Vec::new();
        for (tag, kind, v) in shorts {
            let mut e = tag.to_le_bytes().to_vec();
            e.extend_from_slice(&kind.to_le_bytes());
            e.extend_from_slice(&1u32.to_le_bytes());
            e.extend_from_slice(&v.to_le_bytes());
            all.push((*tag, e));
        }
        for (tag, text) in texts {
            let mut e = tag.to_le_bytes().to_vec();
            e.extend_from_slice(&2u16.to_le_bytes());
            e.extend_from_slice(&(text.len() as u32).to_le_bytes());
            e.extend_from_slice(&(values_at + values.len() as u32).to_le_bytes());
            values.extend_from_slice(text);
            all.push((*tag, e));
        }
        for (tag, _) in pointers {
            let mut e = tag.to_le_bytes().to_vec();
            e.extend_from_slice(&4u16.to_le_bytes());
            e.extend_from_slice(&1u32.to_le_bytes());
            e.extend_from_slice(&0u32.to_le_bytes());
            all.push((*tag, e));
        }
        all.sort_by_key(|(tag, _)| *tag);
        for (tag, e) in all {
            if pointers.iter().any(|(t, _)| *t == tag) {
                pointer_fields.push(out.len() + 8);
            }
            out.extend_from_slice(&e);
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // no next directory
        out.extend_from_slice(&values);
        (at, pointer_fields)
    };
    let shorts = [(256u16, 3u16, 2u32), (257, 3, 2), (259, 3, 1), (262, 3, 2), (273, 4, pixels_at), (277, 3, 3), (278, 3, 2), (279, 4, 12)];
    let (ifd0, fields) = ifd(&mut out, &shorts, &texts, &[(0x8769, 0), (0x8825, 0)]);
    out[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let (exif, _) = ifd(&mut out, &[], &exif_texts, &[]);
    let (gps, _) = ifd(&mut out, &[(0x0000, 1, 0x0202)], &gps_texts, &[]);
    out[fields[0]..fields[0] + 4].copy_from_slice(&exif.to_le_bytes());
    out[fields[1]..fields[1] + 4].copy_from_slice(&gps.to_le_bytes());
    out
}

#[test]
fn a_tiff_keeps_its_image_and_loses_camera_owner_dates_and_position() {
    let tagged = tagged_tiff();
    let (how, out) = scrubbed("photo.tif", &tagged);
    assert_eq!(how, Scrubbed::Cleaned);
    assert_eq!(leaks(&out), Vec::<&str>::new());
    assert_eq!(out.len(), tagged.len(), "patched in place");
    assert_eq!(&out[8..20], &tagged[8..20], "the pixels are the same bytes");
    assert_eq!(&out[..8], &tagged[..8], "the header and the first directory's place are unchanged");
    // A second pass finds only zeros left to clear.
    let mut again = Cursor::new(out.clone());
    for e in tiff::edits(&mut again).unwrap() {
        if let Edit::Zero { offset, len } = e {
            assert!(out[offset as usize..(offset + len) as usize].iter().all(|b| *b == 0));
        }
    }
}

// ---- HEIF ----

fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut b = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(payload);
    b
}

fn full_box(kind: &[u8; 4], version: u8, payload: &[u8]) -> Vec<u8> {
    mp4_box(kind, &[&[version, 0, 0, 0][..], payload].concat())
}

/// A phone's HEIC: an image item, an Exif item and an XMP item, located by iloc in the media data.
fn tagged_heic() -> Vec<u8> {
    let picture: &[u8] = b"HEVC-PICTURE-BYTES-0123456789";
    let exif = [&[0u8, 0, 0, 6][..], b"Exif\0\0", &exif_block()].concat();
    let xmp: &[u8] = b"<x:xmpmeta xmpMM:DocumentID=\"DOC-1234\" photoshop:City=\"Cupertino\"/>";
    let infe = |id: u16, kind: &[u8; 4], extra: &[u8]| full_box(b"infe", 2, &[&id.to_be_bytes()[..], &[0, 0], kind, b"\0", extra].concat());
    let iinf = full_box(
        b"iinf",
        0,
        &[&3u16.to_be_bytes()[..], &infe(1, b"hvc1", b""), &infe(2, b"Exif", b""), &infe(3, b"mime", b"application/rdf+xml\0")].concat(),
    );
    let ftyp = mp4_box(b"ftyp", b"heic\0\0\0\0mif1heic");
    // iloc offsets are filled in once the meta box's size is known.
    let iloc_for = |mdat_at: u32| {
        let mut p = vec![0x44, 0x00]; // offset and length sizes 4, base offset size 0
        p.extend_from_slice(&3u16.to_be_bytes());
        let mut at = mdat_at + 8;
        for (id, data) in [(1u16, picture), (2, exif.as_slice()), (3, xmp)] {
            p.extend_from_slice(&id.to_be_bytes());
            p.extend_from_slice(&0u16.to_be_bytes()); // data reference index
            p.extend_from_slice(&1u16.to_be_bytes()); // one extent
            p.extend_from_slice(&at.to_be_bytes());
            p.extend_from_slice(&(data.len() as u32).to_be_bytes());
            at += data.len() as u32;
        }
        full_box(b"iloc", 0, &p)
    };
    let meta_for = |mdat_at: u32| full_box(b"meta", 0, &[mp4_box(b"hdlr", &[0; 25]), iinf.clone(), iloc_for(mdat_at)].concat());
    let probe = meta_for(0);
    let mdat_at = (ftyp.len() + probe.len()) as u32;
    let mdat = mp4_box(b"mdat", &[picture, &exif, xmp].concat());
    [ftyp, meta_for(mdat_at), mdat].concat()
}

#[test]
fn a_heic_loses_its_exif_and_xmp_items_and_keeps_the_picture() {
    let tagged = tagged_heic();
    assert_eq!(sniff(&tagged), Format::Heif);
    let (_, out) = scrubbed("photo.heic", &tagged);
    assert_eq!(leaks(&out), Vec::<&str>::new());
    assert_eq!(out.len(), tagged.len());
    assert!(contains(&out, b"HEVC-PICTURE-BYTES-0123456789"), "the picture item is untouched");
}

// ---- MP4 / MOV ----

/// A phone video: times in mvhd, tkhd and mdhd, a handler and encoder that name the phone, location and camera in
/// udta, Apple keys in meta, and a top-level XMP box.
fn tagged_mp4() -> Vec<u8> {
    let time = 0x7A11_2233u32.to_be_bytes(); // a 2026 timestamp, as a number
    let mvhd = full_box(b"mvhd", 0, &[&time[..], &time, &[0; 88]].concat());
    let tkhd = full_box(b"tkhd", 0, &[&time[..], &time, &[0; 76]].concat());
    let mdhd = full_box(b"mdhd", 0, &[&time[..], &time, &[0; 12]].concat());
    let hdlr = full_box(b"hdlr", 0, &[&[0u8; 4][..], b"vide", &[0; 12], b"PhoneCo Video Handler\0"].concat());
    let mut avc1 = vec![0u8; 78];
    avc1[12..16].copy_from_slice(b"PhCo");
    avc1[42] = 13;
    avc1[43..56].copy_from_slice(b"CameraApp 9.1");
    let stsd = full_box(b"stsd", 0, &[&1u32.to_be_bytes()[..], &mp4_box(b"avc1", &avc1)].concat());
    let trak = mp4_box(b"trak", &[tkhd, mp4_box(b"mdia", &[mdhd, hdlr, mp4_box(b"minf", &mp4_box(b"stbl", &stsd))].concat())].concat());
    let udta = mp4_box(
        b"udta",
        &[
            mp4_box(b"\xA9xyz", b"\x00\x13\x15\xc7+37.3349-122.0090/"),
            mp4_box(b"\xA9mak", b"\x00\x07\x15\xc7PhoneCo"),
            mp4_box(b"\xA9mod", b"\x00\x08\x15\xc7Phone 15"),
            mp4_box(b"\xA9day", b"\x00\x0a\x15\xc72026-10-29"),
            mp4_box(b"\xA9too", b"\x00\x0d\x15\xc7CameraApp 9.1"),
        ]
        .concat(),
    );
    let key = |name: &str| mp4_box(b"mdta", name.as_bytes());
    let keys = full_box(b"keys", 0, &[&2u32.to_be_bytes()[..], &key("com.apple.quicktime.location.ISO6709"), &key("com.apple.quicktime.creationdate")].concat());
    let data = |text: &str| mp4_box(b"data", &[&[0, 0, 0, 1, 0, 0, 0, 0][..], text.as_bytes()].concat());
    let ilst = mp4_box(b"ilst", &[mp4_box(&1u32.to_be_bytes(), &data("+37.3349-122.0090/")), mp4_box(&2u32.to_be_bytes(), &data("2026-10-29T15:30:00"))].concat());
    let meta = mp4_box(b"meta", &[mp4_box(b"hdlr", &[0; 25]), keys, ilst].concat());
    let moov = mp4_box(b"moov", &[mvhd, trak, udta, meta].concat());
    let xmp = mp4_box(b"uuid", &[&mp4::tests_xmp_uuid()[..], b"<x:xmpmeta dc:creator=\"Maya Doe\"/>"].concat());
    [mp4_box(b"ftyp", b"qt  \0\0\0\0qt  "), moov, xmp, mp4_box(b"mdat", &[0x42; 64])].concat()
}

#[test]
fn a_video_loses_location_device_dates_and_names_in_place() {
    let tagged = tagged_mp4();
    assert!(mp4::has_device_details(&mut Cursor::new(&tagged)).unwrap(), "a phone video counts as personal");
    let (_, out) = scrubbed("video.mov", &tagged);
    assert_eq!(leaks(&out), Vec::<&str>::new());
    assert!(!contains(&out, b"PhCo") && !contains(&out, b"PhoneCo"), "vendor and handler names go");
    assert!(!contains(&out, &0x7A11_2233u32.to_be_bytes()), "every creation and modification time is zero");
    assert_eq!(out.len(), tagged.len(), "no offset in the file moves");
    assert!(contains(&out, &[0x42; 64]), "the media data is untouched");
    assert!(!mp4::has_device_details(&mut Cursor::new(&out)).unwrap());
}

// ---- Matroska ----

fn ebml(id: u32, data: &[u8]) -> Vec<u8> {
    let id_bytes: Vec<u8> = id.to_be_bytes().iter().copied().skip_while(|b| *b == 0).collect();
    let mut e = id_bytes;
    // An 8-byte size, which every element may use.
    let size = (1u64 << 56) | data.len() as u64;
    e.extend_from_slice(&size.to_be_bytes());
    e.extend_from_slice(data);
    e
}

fn tagged_mkv() -> Vec<u8> {
    let info = ebml(
        0x1549_A966,
        &[
            ebml(0x2AD7B1, &[0x0F, 0x42, 0x40]), // TimecodeScale
            ebml(0x7BA9, b"Maya Doe birthday"),
            ebml(0x4461, &[1, 2, 3, 4, 5, 6, 7, 8]),
            ebml(0x4D80, b"CameraApp 9.1"),
            ebml(0x5741, b"John's camera"),
        ]
        .concat(),
    );
    let tracks = ebml(0x1654_AE6B, &ebml(0xAE, &[ebml(0xD7, &[1]), ebml(0x536E, b"Phone 15 front camera")].concat()));
    let tags = ebml(0x1254_C367, b"SimpleTag RECORDING_LOCATION=Cupertino 37.3349 DATE_RECORDED=2026-10-29");
    let attachments = ebml(0x1941_A469, b"cover.jpg Exif Make=PhoneCo SN12345");
    let cluster = ebml(0x1F43_B675, &[0x55; 40]);
    let segment = ebml(0x1853_8067, &[info, tracks, tags, attachments, cluster].concat());
    [ebml(0x1A45_DFA3, &ebml(0x4282, b"webm")), segment].concat()
}

#[test]
fn a_matroska_file_loses_title_dates_apps_names_tags_and_attachments() {
    let tagged = tagged_mkv();
    assert_eq!(sniff(&tagged), Format::Matroska);
    let (_, out) = scrubbed("video.webm", &tagged);
    assert_eq!(leaks(&out), Vec::<&str>::new());
    assert_eq!(out.len(), tagged.len());
    assert!(contains(&out, &[0x55; 40]), "the cluster is untouched");
    assert!(!contains(&out, &[1, 2, 3, 4, 5, 6, 7, 8]), "the date goes");
    assert!(mkv::edits(&mut Cursor::new(&out)).unwrap().iter().all(|e| matches!(e, Edit::Bytes { bytes, .. } if bytes[0] == 0xEC) || matches!(e, Edit::Zero { .. })), "only Voids are left");
}

// ---- failures and fallbacks ----

#[test]
fn an_unknown_format_is_refused_and_a_broken_picture_that_decodes_is_reencoded() {
    let dir = scratch("fail");
    let odd = dir.join("photo.bmp");
    std::fs::write(&odd, b"BM not a picture this build decodes, by Maya Doe").unwrap();
    assert!(scrub(&odd, &dir.join("out.bmp")).is_err(), "a format that can be neither cleaned nor decoded is refused");

    // A GIF with a block this does not know after its frame: it cannot be cleaned block by block, but it decodes.
    let clean = picture(image::ImageFormat::Gif);
    let mut broken = clean[..clean.len() - 1].to_vec();
    broken.extend_from_slice(b"\x99 by Maya Doe");
    let src = dir.join("photo.gif");
    std::fs::write(&src, &broken).unwrap();
    let out = dir.join("photo-out.png");
    match scrub(&src, &out) {
        Ok(Scrubbed::ReencodedPng) => {
            let png = std::fs::read(&out).unwrap();
            assert!(png.starts_with(b"\x89PNG") && leaks(&png).is_empty());
            assert_eq!(decodes(&png), (16, 8));
        }
        other => panic!("expected a PNG re-encode, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inspection_gives_the_defaults() {
    let dir = scratch("inspect");
    let photo = dir.join("a.jpg");
    std::fs::write(&photo, tagged_jpeg()).unwrap();
    assert_eq!(inspect(&photo, "image/jpeg"), Inspection { kind: MediaKind::Photo, identifying: true, removable: true });
    let phone_video = dir.join("b.mov");
    std::fs::write(&phone_video, tagged_mp4()).unwrap();
    assert_eq!(inspect(&phone_video, "video/quicktime"), Inspection { kind: MediaKind::Video, identifying: true, removable: true });
    let movie = dir.join("c.webm");
    std::fs::write(&movie, tagged_mkv()).unwrap();
    assert_eq!(inspect(&movie, "video/webm"), Inspection { kind: MediaKind::Video, identifying: false, removable: true });
    let song = dir.join("d.mp3");
    std::fs::write(&song, b"ID3\x03\0\0\0\0\0\x10TPE1 Maya Doe").unwrap();
    assert_eq!(inspect(&song, "audio/mpeg"), Inspection { kind: MediaKind::Audio, identifying: false, removable: false });
    let doc = dir.join("e.pdf");
    std::fs::write(&doc, b"%PDF-1.7 /Author (Maya Doe)").unwrap();
    assert_eq!(inspect(&doc, "application/pdf").kind, MediaKind::Other);
    let _ = std::fs::remove_dir_all(&dir);
}
