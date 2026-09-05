use std::io::{Cursor, Write};

/// A Photoshop resource block, including the even-padded Pascal name and payload.
pub fn photoshop_resource(id: u16, jpeg: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut resource = b"8BIM".to_vec();
    resource.extend(id.to_be_bytes());
    resource.extend([3, b'p', b'r', b'e']);
    resource.extend((28u32 + jpeg.len() as u32).to_be_bytes());
    for value in [1, width, height, width * 3, width * height * 3, jpeg.len() as u32] {
        resource.extend(value.to_be_bytes());
    }
    resource.extend(24u16.to_be_bytes());
    resource.extend(1u16.to_be_bytes());
    resource.extend(jpeg);
    if resource.len() % 2 != 0 { resource.push(0); }
    resource
}

pub fn jpeg(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb(color));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Jpeg).unwrap();
    bytes.into_inner()
}

/// Big-endian, 16-bit RGB, Deflate strips of 37 rows, matching the real sample's
/// layout. Metadata and pixel payloads are disjoint so tests can remove either.
pub fn deflate_rgb16(width: u32, height: u32, resource: Option<&[u8]>) -> Vec<u8> {
    let rows = 37;
    let mut strips = Vec::new();
    for top in (0..height).step_by(rows as usize) {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        for y in top..(top + rows).min(height) {
            for x in 0..width {
                // Low bytes contain detail, rather than a trivially compressible
                // constant: exercise 16-bit conversion and realistic decode work.
                let noise = x.wrapping_mul(2654435761) ^ y.wrapping_mul(2246822519);
                for (channel, high) in [180u16, 90, 30].into_iter().enumerate() {
                    let pixel = high << 8 | ((noise >> (channel * 8)) & 255) as u16;
                    encoder.write_all(&pixel.to_be_bytes()).unwrap();
                }
            }
        }
        strips.push(encoder.finish().unwrap());
    }
    let shorts = |values: &[u16]| values.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<_>>();
    let longs = |values: &[u32]| values.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<_>>();
    let count = strips.len() as u32;
    let mut tags = vec![
        (256u16, 4u16, 1, longs(&[width])),
        (257, 4, 1, longs(&[height])),
        (258, 3, 3, shorts(&[16, 16, 16])),
        (259, 3, 1, shorts(&[8])),
        (262, 3, 1, shorts(&[2])),
        (273, 4, count, longs(&vec![0; strips.len()])),
        (274, 3, 1, shorts(&[1])),
        (277, 3, 1, shorts(&[3])),
        (278, 4, 1, longs(&[rows])),
        (279, 4, count, longs(&strips.iter().map(|s| s.len() as u32).collect::<Vec<_>>())),
        (284, 3, 1, shorts(&[1])),
    ];
    if let Some(resource) = resource { tags.push((34377, 1, resource.len() as u32, resource.to_vec())); }
    let mut bytes = b"MM\0*\0\0\0\x08".to_vec();
    bytes.extend((tags.len() as u16).to_be_bytes());
    bytes.resize(8 + 2 + tags.len() * 12 + 4, 0);
    let mut offsets_position = 0;
    for (index, (tag, kind, count, data)) in tags.into_iter().enumerate() {
        let entry = 10 + index * 12;
        bytes[entry..entry + 2].copy_from_slice(&tag.to_be_bytes());
        bytes[entry + 2..entry + 4].copy_from_slice(&kind.to_be_bytes());
        bytes[entry + 4..entry + 8].copy_from_slice(&count.to_be_bytes());
        let position = if data.len() <= 4 { entry + 8 } else {
            let offset = bytes.len();
            bytes[entry + 8..entry + 12].copy_from_slice(&(offset as u32).to_be_bytes());
            bytes.resize(offset + data.len(), 0);
            offset
        };
        bytes[position..position + data.len()].copy_from_slice(&data);
        if tag == 273 { offsets_position = position; }
        if bytes.len() % 2 != 0 { bytes.push(0); }
    }
    for (index, strip) in strips.into_iter().enumerate() {
        let position = offsets_position + index * 4;
        let offset = bytes.len() as u32;
        bytes[position..position + 4].copy_from_slice(&offset.to_be_bytes());
        bytes.extend(strip);
    }
    bytes
}
