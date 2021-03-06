//! # WAV
//!
//! This is a crate for reading in and writing out wave files. It supports bit-
//! depths of 8, 16, and 24 bits, any number of channels, and uncompressed PCM
//! data. Unfortunately other types of data format (e.g. compressed WAVE files)
//! are not supported. There is also no support for any metadata chunks or any
//! chunks other than the "fmt " and "data" chunks.
//!
//! ## Example
//!
//! ```rust
//! # fn main() -> std::io::Result<()> {
//! use std::fs::File;
//! use std::path::Path;
//!
//! let mut inp_file = File::open(Path::new("data/sine.wav"))?;
//! let (header, data) = wav::read(&mut inp_file)?;
//!
//! let mut out_file = File::create(Path::new("data/output.wav"))?;
//! wav::write(header, &data, &mut out_file)?;
//! # Ok(())
//! # }
//! ```

#![deny(broken_intra_doc_links)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]

use riff;
use std::{
    convert::TryFrom,
    io::{self, Read, Write},
};

pub mod header;
pub use header::Header;

pub mod bit_depth;
pub use bit_depth::BitDepth;

mod tuple_iterator;
use tuple_iterator::{PairIter, TripletIter};

/// Reads in the given `reader` and attempts to extract the audio data and
/// header from it.
///
/// ## Errors
///
/// This function fails under the following circumstances:
/// * Any error occurring from the `reader` parameter during reading.
/// * The data isn't RIFF data.
/// * The wave header specifies a compressed data format.
/// * The wave header specifies an unsupported bit-depth.
/// * The wave data is malformed, or otherwise couldn't be parsed into samples.
pub fn read<R>(reader: &mut R) -> io::Result<(Header, BitDepth)>
where
    R: Read + io::Seek,
{
    let header = read_header(reader)?;
    Ok((header, read_data(reader, &header)?))
}

/// Writes the given wav data to the given `writer`.
///
/// ## Notes
///
/// Although `track` is a borrowed value, its contents will be formatted into an
/// owned `Vec<u8>` so that it can be written to the `writer` through
/// [`riff::write_chunk`][0].
///
/// ## Errors
///
/// This function fails under the following circumstances:
/// * Any error occurring from the `writer` parameter during writing.
/// * The given BitDepth is `BitDepth::Empty`.
///
/// [0]: riff::write_chunk
pub fn write<W>(header: Header, track: &BitDepth, writer: &mut W) -> std::io::Result<()>
where
    W: Write + io::Seek
{
    let w_id = riff::ChunkId::new("WAVE").unwrap();

    let h_id = riff::ChunkId::new("fmt ").unwrap();
    let h_vec: [u8; 16] = header.into();
    let h_dat = riff::ChunkContents::Data(h_id, Vec::from(&h_vec[0..16]));

    let d_id = riff::ChunkId::new("data").unwrap();
    let d_vec = match track {
        BitDepth::Eight(v) => v.clone(),
        BitDepth::Sixteen(v) => v.iter()
            .flat_map(
                |s| {
                    let v = s.to_le_bytes();
                    PairIter::new((v[0], v[1]))
                }
            )
            .collect::<Vec<_>>(),
        BitDepth::TwentyFour(v) => v.iter()
            .flat_map(
                |s| {
                    let v = s.to_le_bytes().split_at(1).1.to_owned();
                    TripletIter::new((v[0], v[1], v[2]))
                }
            )
            .collect::<Vec<_>>(),
        _ => return Err(
            std::io::Error::new(
                std::io::ErrorKind::Other,
                "Empty audio data given",
            )
        ),
    };
    let d_dat = riff::ChunkContents::Data(d_id, d_vec);

    let r = riff::ChunkContents::Children(riff::RIFF_ID.clone(), w_id, vec![h_dat, d_dat]);

    r.write(writer)?;

    Ok(())
}

fn read_header<R>(reader: &mut R) -> io::Result<Header>
where
    R: Read + io::Seek,
{
    let wav = verify_wav_file(reader)?;

    for c in wav.iter(reader) {
        if c.id().as_str() == "fmt " {
            // Read header contents
            let header_bytes = c.read_contents(reader)?;
            let header = Header::try_from(header_bytes.as_slice())
                .map_err(
                    |e| io::Error::new(
                        io::ErrorKind::Other,
                        e
                    )
                )?;

            // Return error if not using PCM
            if header.audio_format != 1 {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Unsupported data format, data is not in uncompressed PCM format, aborting",
                ));
            }

            return Ok(header);
        }
    }

    Err(
        io::Error::new(
            io::ErrorKind::InvalidData,
            "RIFF data is missing the \"fmt \" chunk, aborting"
        )
    )
}

fn read_data<R>(reader: &mut R, header: &Header) -> io::Result<BitDepth>
where
    R: Read + io::Seek,
{
    let wav = verify_wav_file(reader)?;

    for c in wav.iter(reader) {
        if c.id().as_str() == "data" {
            // Read data contents
            let data_bytes = c.read_contents(reader)?;

            return Ok(
                match header.bits_per_sample {
                    8 => BitDepth::Eight(data_bytes),
                    16 => BitDepth::Sixteen({
                        let mut tmpv = Vec::with_capacity(data_bytes.len() / 2);
                        tmpv.extend(data_bytes.chunks_exact(2).map(|i| i16::from_le_bytes([i[0], i[1]])));
                        tmpv
                    }),
                    24 => BitDepth::TwentyFour({
                        let mut tmpv = Vec::with_capacity(data_bytes.len() / 3);
                        tmpv.extend(data_bytes.chunks_exact(3).map(|i| i32::from_le_bytes([0, i[0], i[1], i[2]])));
                        tmpv
                    }),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            "Unsupported bit depth",
                        ));
                    }
                }
            );
        }
    }

    Err(
        io::Error::new(
            io::ErrorKind::Other,
            "Could not parse audio data",
        )
    )
}

fn verify_wav_file<R>(reader: &mut R) -> io::Result<riff::Chunk>
where
    R: Read + io::Seek,
{
    let wav = riff::Chunk::read(reader, 0)?;

    let form_type = wav.read_type(reader)?;

    if form_type.as_str() != "WAVE" {
        Err(
            io::Error::new(
                io::ErrorKind::Other,
                "RIFF file type not \"WAVE\"",
            )
        )
    } else {
        Ok(wav)
    }
}
