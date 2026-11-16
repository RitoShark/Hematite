use anyhow::{ensure, Result};
use rs_audio::{Bnk, Wem, WemCodec, Wpk};
use rs_io::{Parse, Serialize};

pub fn validate_bank(bytes: &[u8]) -> Result<()> {
    if bytes.starts_with(b"r3d2") {
        let bank = Wpk::from_bytes(bytes)?;
        for entry in &bank.entries {
            Wem::new(&entry.data)?;
        }
        return Ok(());
    }
    let bank = Bnk::from_bytes(bytes)?;
    ensure!(
        bank.version().is_some() && bank.bank_id().is_some(),
        "incomplete bank header"
    );
    bank.validate_media()?;
    bank.hirc()?;
    for (_, bytes) in bank.wems() {
        Wem::new(bytes)?;
    }
    Ok(())
}

fn compress_wem(bytes: &[u8]) -> Result<Option<Vec<u8>>> {
    let wem = Wem::new(bytes)?;
    if wem.format().codec != WemCodec::Vorbis {
        return Ok(None);
    }
    if wem.has_extra_chunks() {
        return Ok(None);
    }
    let pcm = wem.to_pcm()?;
    let compressed = rs_audio::encode_vorbis_like(bytes, &pcm, 0.4)?;
    let decoded = Wem::new(&compressed)?.to_pcm()?;
    ensure!(
        decoded.sample_rate == pcm.sample_rate
            && decoded.channels == pcm.channels
            && decoded.frames() == pcm.frames(),
        "Recompression changed audio rate, channels, or duration"
    );
    Ok((compressed.len() < bytes.len()).then_some(compressed))
}

pub fn compress_bank(bytes: &[u8]) -> Result<Vec<u8>> {
    validate_bank(bytes)?;
    let mut changed = false;
    let output = if bytes.starts_with(b"r3d2") {
        let mut bank = Wpk::from_bytes(bytes)?;
        for entry in &mut bank.entries {
            if let Some(compressed) = compress_wem(&entry.data)? {
                entry.data = compressed;
                changed = true;
            }
        }
        bank.to_bytes()?
    } else {
        let mut bank = Bnk::from_bytes(bytes)?;
        let mut replacements = Vec::new();
        for (id, original) in bank.wems() {
            if let Some(compressed) = compress_wem(original)? {
                replacements.push((id, compressed));
            }
        }
        changed = !replacements.is_empty();
        bank.replace_wems(replacements)?;
        bank.to_bytes()?
    };
    if !changed || output.len() >= bytes.len() {
        return Ok(bytes.to_vec());
    }
    validate_bank(&output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs_audio::{BnkSection, PcmAudio};

    #[test]
    fn leaves_pcm_media_and_bank_metadata_unchanged() {
        let mut bank = Bnk {
            sections: vec![
                BnkSection {
                    tag: *b"BKHD",
                    data: [145u32.to_le_bytes(), 99u32.to_le_bytes()].concat(),
                },
                BnkSection {
                    tag: *b"TEST",
                    data: vec![9, 8, 7],
                },
            ],
        };
        bank.insert_wem(
            42,
            rs_audio::encode_pcm(&PcmAudio::silence(44100, 1, 4096)).unwrap(),
        )
        .unwrap();
        let bytes = bank.to_bytes().unwrap();
        assert_eq!(compress_bank(&bytes).unwrap(), bytes);
    }

    #[test]
    fn rejects_out_of_bounds_media_index() {
        let bank = Bnk {
            sections: vec![
                BnkSection {
                    tag: *b"BKHD",
                    data: [145u32.to_le_bytes(), 99u32.to_le_bytes()].concat(),
                },
                BnkSection {
                    tag: *b"DIDX",
                    data: vec![0; 12],
                },
            ],
        };
        assert!(validate_bank(&bank.to_bytes().unwrap()).is_err());
    }
    #[test]
    fn vorbis_recompression_preserves_container_metadata_and_decodable_duration() {
        let samples = (0..44100)
            .map(|i| ((i as f32 * 0.073).sin() * 24000.0) as i16)
            .collect();
        let original = rs_audio::encode_vorbis(&PcmAudio::new(44100, 1, samples), 0.9).unwrap();
        let pcm = Wem::new(&original).unwrap().to_pcm().unwrap();
        let header = [145u32.to_le_bytes(), 99u32.to_le_bytes()].concat();
        let mut bank = Bnk {
            sections: vec![
                BnkSection {
                    tag: *b"BKHD",
                    data: header.clone(),
                },
                BnkSection {
                    tag: *b"TEST",
                    data: vec![9, 8, 7],
                },
                BnkSection {
                    tag: *b"HIRC",
                    data: 0u32.to_le_bytes().to_vec(),
                },
            ],
        };
        bank.insert_wem(42, original.clone()).unwrap();
        let input = bank.to_bytes().unwrap();
        let output = compress_bank(&input).unwrap();
        assert!(
            output.len() < input.len(),
            "fixture should exercise a real recompression"
        );
        let read = Bnk::from_bytes(&output).unwrap();
        assert_eq!(read.bank_id(), Some(99));
        assert_eq!(read.wem_ids(), vec![42]);
        assert_eq!(
            read.sections
                .iter()
                .find(|s| s.tag == *b"BKHD")
                .unwrap()
                .data,
            header
        );
        assert_eq!(
            read.sections
                .iter()
                .find(|s| s.tag == *b"TEST")
                .unwrap()
                .data,
            vec![9, 8, 7]
        );
        assert_eq!(
            read.sections
                .iter()
                .find(|s| s.tag == *b"HIRC")
                .unwrap()
                .data,
            0u32.to_le_bytes()
        );
        let compressed = Wem::new(read.wem(42).unwrap()).unwrap();
        assert_eq!(compressed.format().codec, WemCodec::Vorbis);
        assert_eq!(compressed.to_pcm().unwrap().frames(), pcm.frames());
        let package = Wpk {
            version: 1,
            entries: vec![rs_audio::WemEntry::new("42.wem", original)],
            dead_slots: vec![0],
        };
        let output = compress_bank(&package.to_bytes().unwrap()).unwrap();
        let read = Wpk::from_bytes(&output).unwrap();
        assert_eq!(read.dead_slots, vec![0]);
        assert_eq!(read.entries[0].name, "42.wem");
        assert_eq!(
            Wem::new(&read.entries[0].data)
                .unwrap()
                .to_pcm()
                .unwrap()
                .frames(),
            pcm.frames()
        );
    }

    #[test]
    fn preserves_streams_with_extra_chunks() {
        let mut bytes = rs_audio::encode_vorbis(&PcmAudio::silence(44100, 1, 4096), 0.9).unwrap();
        bytes.extend_from_slice(b"cue ");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let length = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        assert!(compress_wem(&bytes).unwrap().is_none());
    }
}
