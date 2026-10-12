//! Merge separately packaged fMP4 tracks into one fragmented MP4 file.
//!
//! ```sh
//! cargo run -p mp4 --example merge_fmp4 -- out.mp4 \
//!     video/init.mp4 video/1.m4s video/2.m4s -- audio/init.mp4 audio/1.m4s audio/2.m4s
//! ```
//!
//! Each `--`-separated group is one track: its initialization segment, then
//! its media segments in order. Fragments are written in decode-time order
//! across tracks. Useful for checking the merger against real streams, e.g.
//! `ffmpeg -v error -i out.mp4 -f null -`.

use std::error::Error;
use std::fs;

use bytes::Bytes;
use mp4::merge::{FragmentWriter, SourceFragment, merge_init};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .ok_or("usage: merge_fmp4 <output> <init> <segment>... [-- <init> <segment>...]...")?;
    let rest: Vec<String> = args.collect();
    let groups: Vec<&[String]> = rest.split(|arg| arg == "--").collect();

    let mut inits = Vec::new();
    for group in &groups {
        let init = group
            .first()
            .ok_or("every track needs an initialization segment")?;
        inits.push(Bytes::from(fs::read(init)?));
    }
    let merged = merge_init(&inits)?;

    let mut fragments: Vec<SourceFragment> = Vec::new();
    for (track, group) in groups.iter().enumerate() {
        for path in &group[1..] {
            let segment = Bytes::from(fs::read(path)?);
            fragments.extend(
                merged
                    .parse_media_segment(track, &segment, 0)
                    .map_err(|error| format!("{path}: {error}"))?,
            );
        }
    }
    // Compare decode times across timescales exactly: t1 / s1 vs t2 / s2.
    let scaled = |fragment: &SourceFragment, other: &SourceFragment| {
        let timescale = merged.tracks()[other.track_index()].timescale();
        u128::from(fragment.decode_time()) * u128::from(timescale)
    };
    fragments.sort_by(|a, b| {
        scaled(a, b)
            .cmp(&scaled(b, a))
            .then(a.track_index().cmp(&b.track_index()))
    });

    let mut out = merged.data().to_vec();
    let mut writer = FragmentWriter::new();
    let count = fragments.len();
    for fragment in fragments {
        writer.write(fragment, &mut out)?;
    }
    fs::write(&output, &out)?;
    eprintln!(
        "wrote {output}: {} tracks, {count} fragments, {} bytes",
        merged.tracks().len(),
        out.len()
    );
    Ok(())
}
