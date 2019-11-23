use byteorder::{ReadBytesExt, WriteBytesExt, LE};
use encoding_rs::EUC_KR;

use std::io::{self, Read, Write};
use std::num::NonZeroU64;

use crate::constants::PK2_FILE_ENTRY_SIZE;
use crate::error::{Error, Pk2Result};
use crate::ChainIndex;
use crate::FILETIME;

/// An entry of a [`PackBlock`].
#[derive(Clone, Eq, PartialEq)]
pub(crate) enum PackEntry {
    Empty {
        next_block: Option<NonZeroU64>,
    },
    Directory {
        name: String,
        access_time: FILETIME,
        create_time: FILETIME,
        modify_time: FILETIME,
        pos_children: ChainIndex,
        next_block: Option<NonZeroU64>,
    },
    File {
        name: String,
        access_time: FILETIME,
        create_time: FILETIME,
        modify_time: FILETIME,
        pos_data: u64,
        size: u32,
        next_block: Option<NonZeroU64>,
    },
}

impl Default for PackEntry {
    fn default() -> Self {
        PackEntry::Empty { next_block: None }
    }
}

impl PackEntry {
    pub(crate) fn new_directory(
        name: String,
        pos_children: ChainIndex,
        next_block: Option<NonZeroU64>,
    ) -> Self {
        let ftime = FILETIME::now();
        PackEntry::Directory {
            name,
            access_time: ftime,
            create_time: ftime,
            modify_time: ftime,
            pos_children,
            next_block,
        }
    }

    pub(crate) fn new_file(
        name: String,
        pos_data: u64,
        size: u32,
        next_block: Option<NonZeroU64>,
    ) -> Self {
        let ftime = FILETIME::now();
        PackEntry::File {
            name,
            access_time: ftime,
            create_time: ftime,
            modify_time: ftime,
            pos_data,
            size,
            next_block,
        }
    }

    pub(crate) fn clear(&mut self) {
        let next_block = match *self {
            PackEntry::Empty { next_block }
            | PackEntry::Directory { next_block, .. }
            | PackEntry::File { next_block, .. } => next_block,
        };
        *self = PackEntry::Empty { next_block };
    }

    pub(crate) fn next_block(&self) -> Option<NonZeroU64> {
        match *self {
            PackEntry::Empty { next_block }
            | PackEntry::Directory { next_block, .. }
            | PackEntry::File { next_block, .. } => next_block,
        }
    }

    pub(crate) fn set_next_block(&mut self, nc: u64) {
        match self {
            PackEntry::Empty { .. } => (),
            PackEntry::Directory { next_block, .. } | PackEntry::File { next_block, .. } => {
                *next_block = NonZeroU64::new(nc)
            }
        }
    }

    pub(crate) fn name(&self) -> Option<&str> {
        match self {
            PackEntry::Empty { .. } => None,
            PackEntry::Directory { name, .. } | PackEntry::File { name, .. } => Some(name),
        }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            PackEntry::Empty { .. } => true,
            _ => false,
        }
    }

    #[inline]
    pub(crate) fn is_file(&self) -> bool {
        match self {
            PackEntry::File { .. } => true,
            _ => false,
        }
    }

    #[inline]
    pub(crate) fn is_dir(&self) -> bool {
        match self {
            PackEntry::Directory { .. } => true,
            _ => false,
        }
    }
}

impl PackEntry {
    // Will always seek to the end of the entry
    pub(crate) fn from_reader<R: Read>(mut r: R) -> Pk2Result<Self> {
        match r.read_u8()? {
            0 => {
                r.read_exact(&mut [0; PK2_FILE_ENTRY_SIZE - 1])?; //seek to end of entry
                Ok(PackEntry::Empty { next_block: None })
            }
            ty @ 1 | ty @ 2 => {
                let name = {
                    let mut buf = [0; 81];
                    r.read_exact(&mut buf)?;
                    let end = buf
                        .iter()
                        .position(|b| *b == 0)
                        .unwrap_or_else(|| buf.len());
                    EUC_KR
                        .decode_without_bom_handling(&buf[..end])
                        .0
                        .into_owned()
                };
                let access_time = FILETIME {
                    dwLowDateTime: r.read_u32::<LE>()?,
                    dwHighDateTime: r.read_u32::<LE>()?,
                };
                let create_time = FILETIME {
                    dwLowDateTime: r.read_u32::<LE>()?,
                    dwHighDateTime: r.read_u32::<LE>()?,
                };
                let modify_time = FILETIME {
                    dwLowDateTime: r.read_u32::<LE>()?,
                    dwHighDateTime: r.read_u32::<LE>()?,
                };
                let position = r.read_u64::<LE>()?;
                let size = r.read_u32::<LE>()?;
                let next_block = NonZeroU64::new(r.read_u64::<LE>()?);
                r.read_u16::<LE>()?; //padding

                Ok(if ty == 1 {
                    PackEntry::Directory {
                        name,
                        access_time,
                        create_time,
                        modify_time,
                        pos_children: ChainIndex(position),
                        next_block,
                    }
                } else {
                    PackEntry::File {
                        name,
                        access_time,
                        create_time,
                        modify_time,
                        pos_data: position,
                        size,
                        next_block,
                    }
                })
            }
            _ => Err(Error::CorruptedFile),
        }
    }

    pub(crate) fn to_writer<W: Write>(&self, mut w: W) -> io::Result<()> {
        match self {
            PackEntry::Empty { next_block } => {
                w.write_all(&[0; PK2_FILE_ENTRY_SIZE - 8])?;
                w.write_u64::<LE>(next_block.map_or(0, |nc| nc.get()))
                    .map_err(Into::into)
            }
            PackEntry::Directory {
                name,
                access_time,
                create_time,
                modify_time,
                pos_children: ChainIndex(position),
                next_block,
            }
            | PackEntry::File {
                name,
                access_time,
                create_time,
                modify_time,
                pos_data: position,
                next_block,
                ..
            } => {
                w.write_u8(if self.is_dir() { 1 } else { 2 })?;
                let mut encoded = EUC_KR.encode(name).0.into_owned();
                encoded.resize(81, 0);
                w.write_all(&encoded)?;
                w.write_u32::<LE>(access_time.dwLowDateTime)?;
                w.write_u32::<LE>(access_time.dwHighDateTime)?;
                w.write_u32::<LE>(create_time.dwLowDateTime)?;
                w.write_u32::<LE>(create_time.dwHighDateTime)?;
                w.write_u32::<LE>(modify_time.dwLowDateTime)?;
                w.write_u32::<LE>(modify_time.dwHighDateTime)?;
                w.write_u64::<LE>(*position)?;
                w.write_u32::<LE>(if let PackEntry::File { size, .. } = self {
                    *size
                } else {
                    0
                })?;
                w.write_u64::<LE>(next_block.map_or(0, |nc| nc.get()))?;
                w.write_u16::<LE>(0)?;
                Ok(())
            }
        }
    }
}
