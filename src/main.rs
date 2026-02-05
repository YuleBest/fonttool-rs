use anyhow::{Result};
use clap::{Parser, Subcommand};
use std::{
    fs,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use ttf_parser::name_id;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

#[derive(Parser)]
#[command(name = "fonttool-rs", about = "简单的字体处理工具，只有两个功能")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 提取 NameID 1 和 16 并去重
    Getname { #[arg(short, long)] input: PathBuf },
    /// 拆分 TTC 为多个 TTF
    Split { #[arg(short, long)] input: PathBuf, #[arg(short, long)] output: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Getname { input }) => {
            let data = fs::read(&input)?;
            let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
            let mut all_names = Vec::new();
            for i in 0..count {
                if let Ok(face) = ttf_parser::Face::parse(&data, i) {
                    all_names.extend(
                        face.names()
                            .into_iter()
                            .filter(|n| n.name_id == 1 || n.name_id == 16)
                            .filter_map(|n| n.to_string()),
                    );
                }
            }
            all_names.sort();
            all_names.dedup();
            all_names.iter().for_each(|n| println!("{}", n));
        }
        Some(Commands::Split { input, output }) => {
            split_ttc(&input, &output)?;
        }
        None => anyhow::bail!("Please specify a command"),
    }
    Ok(())
}

fn split_ttc(ttc_path: &Path, out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    let data = fs::read(ttc_path)?;
    let mut cursor = Cursor::new(&data);

    let mut signature = [0u8; 4];
    cursor.read_exact(&mut signature)?;
    // if &signature != b"TTCF" {
    //     anyhow::bail!("Not a TTC file");
    // }

    let _version = cursor.read_u32::<BigEndian>()?;
    let num_fonts = cursor.read_u32::<BigEndian>()?;

    let mut offsets = Vec::new();
    for _ in 0..num_fonts {
        offsets.push(cursor.read_u32::<BigEndian>()?);
    }

    for (i, &offset) in offsets.iter().enumerate() {
        cursor.seek(SeekFrom::Start(offset as u64))?;

        let sfnt_version = cursor.read_u32::<BigEndian>()?;
        let num_tables = cursor.read_u16::<BigEndian>()?;
        let search_range = cursor.read_u16::<BigEndian>()?;
        let entry_selector = cursor.read_u16::<BigEndian>()?;
        let range_shift = cursor.read_u16::<BigEndian>()?;

        #[derive(Clone)]
        struct TableRecord {
            tag: u32,
            checksum: u32,
            offset: u32,
            length: u32,
        }

        let mut tables = Vec::new();
        for _ in 0..num_tables {
            let tag = cursor.read_u32::<BigEndian>()?;
            let checksum = cursor.read_u32::<BigEndian>()?;
            let table_offset = cursor.read_u32::<BigEndian>()?;
            let length = cursor.read_u32::<BigEndian>()?;
            tables.push(TableRecord {
                tag,
                checksum,
                offset: table_offset,
                length,
            });
        }

        // 构建新的 TTF 数据
        let mut out_buf = Cursor::new(Vec::new());
        out_buf.write_u32::<BigEndian>(sfnt_version)?;
        out_buf.write_u16::<BigEndian>(num_tables)?;
        out_buf.write_u16::<BigEndian>(search_range)?;
        out_buf.write_u16::<BigEndian>(entry_selector)?;
        out_buf.write_u16::<BigEndian>(range_shift)?;

        let dir_start = out_buf.position();
        out_buf.seek(SeekFrom::Current((num_tables * 16) as i64))?;

        let mut new_tables = Vec::new();
        for table in &tables {
            let pos = out_buf.position() as u32;
            out_buf.seek(SeekFrom::Start(pos as u64))?;
            out_buf.write_all(&data[table.offset as usize..(table.offset + table.length) as usize])?;
            let pad = (4 - (table.length % 4)) % 4;
            for _ in 0..pad { out_buf.write_u8(0)?; }
            new_tables.push(TableRecord {
                tag: table.tag,
                checksum: table.checksum,
                offset: pos,
                length: table.length,
            });
        }

        // 写入表目录
        out_buf.seek(SeekFrom::Start(dir_start))?;
        for table in &new_tables {
            out_buf.write_u32::<BigEndian>(table.tag)?;
            out_buf.write_u32::<BigEndian>(table.checksum)?;
            out_buf.write_u32::<BigEndian>(table.offset)?;
            out_buf.write_u32::<BigEndian>(table.length)?;
        }

        // 解析 Full Name
        let face = ttf_parser::Face::parse(&out_buf.get_ref(), 0)?;
        let full_name = face.names()
            .into_iter()
            .find(|n| n.name_id == name_id::FULL_NAME)
            .and_then(|n| n.to_string())
            .unwrap_or_else(|| format!("subfont_{}", i));

        let safe_name: String = full_name.chars()
            .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
            .collect();

        let out_file = out_dir.join(format!("{}.ttf", safe_name));
        fs::write(&out_file, out_buf.into_inner())?;
        println!("Extracted font {} -> {:?}", i, out_file);
    }

    Ok(())
}