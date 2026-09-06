//! A bounded Thrift reader for the footer and page headers.

use super::{
    MAX_THRIFT_NESTING, TFieldIdentifier, TInputProtocol, TListIdentifier, TMapIdentifier,
    TMessageIdentifier, TSetIdentifier, TStructIdentifier, TType,
};

pub(super) struct BoundedCompactInput<'a> {
    pub(super) remaining: &'a [u8],
    pub(super) last_field_id: i16,
    pub(super) field_stack: [i16; MAX_THRIFT_NESTING],
    pub(super) struct_depth: usize,
    pub(super) nesting: usize,
    pub(super) collection_items: usize,
    pub(super) pending_bool: Option<bool>,
}

impl TInputProtocol for BoundedCompactInput<'_> {
    fn read_message_begin(&mut self) -> thrift::Result<TMessageIdentifier> {
        Err(not_implemented(
            "messages are not valid in Parquet metadata",
        ))
    }

    fn read_message_end(&mut self) -> thrift::Result<()> {
        Err(not_implemented(
            "messages are not valid in Parquet metadata",
        ))
    }

    fn read_struct_begin(&mut self) -> thrift::Result<Option<TStructIdentifier>> {
        self.enter()?;
        if self.struct_depth >= self.field_stack.len() {
            return Err(invalid_data("compact metadata nesting is too deep"));
        }
        self.field_stack[self.struct_depth] = self.last_field_id;
        self.struct_depth += 1;
        self.last_field_id = 0;
        Ok(None)
    }

    fn read_struct_end(&mut self) -> thrift::Result<()> {
        self.struct_depth = self
            .struct_depth
            .checked_sub(1)
            .ok_or_else(|| invalid_data("unbalanced compact struct"))?;
        self.last_field_id = self.field_stack[self.struct_depth];
        self.leave()
    }

    fn read_field_begin(&mut self) -> thrift::Result<TFieldIdentifier> {
        if self.pending_bool.is_some() {
            return Err(invalid_data("compact boolean field was not consumed"));
        }
        let header = self.read_byte()?;
        let delta = i16::from((header & 0xf0) >> 4);
        let field_type = match header & 0x0f {
            1 => {
                self.pending_bool = Some(true);
                TType::Bool
            }
            2 => {
                self.pending_bool = Some(false);
                TType::Bool
            }
            value => compact_type(value)?,
        };
        if field_type == TType::Stop {
            return Ok(TFieldIdentifier {
                name: None,
                field_type,
                id: None,
            });
        }
        self.last_field_id = if delta == 0 {
            self.read_i16()?
        } else {
            self.last_field_id
                .checked_add(delta)
                .ok_or_else(|| invalid_data("compact field id overflow"))?
        };
        Ok(TFieldIdentifier {
            name: None,
            field_type,
            id: Some(self.last_field_id),
        })
    }

    fn read_field_end(&mut self) -> thrift::Result<()> {
        Ok(())
    }

    fn read_bool(&mut self) -> thrift::Result<bool> {
        if let Some(value) = self.pending_bool.take() {
            return Ok(value);
        }
        match self.read_byte()? {
            1 => Ok(true),
            2 => Ok(false),
            _ => Err(invalid_data("invalid compact boolean")),
        }
    }

    fn read_bytes(&mut self) -> thrift::Result<Vec<u8>> {
        let len = usize::try_from(self.read_vlq()?)
            .map_err(|_error| invalid_data("binary length exceeds usize"))?;
        let bytes = self
            .remaining
            .get(..len)
            .ok_or_else(|| end_of_file("binary field exceeds remaining input"))?;
        self.remaining = &self.remaining[len..];
        Ok(bytes.to_vec())
    }

    fn read_i8(&mut self) -> thrift::Result<i8> {
        Ok(i8::from_ne_bytes([self.read_byte()?]))
    }

    fn read_i16(&mut self) -> thrift::Result<i16> {
        i16::try_from(self.read_zigzag()?)
            .map_err(|_error| invalid_data("compact i16 is out of range"))
    }

    fn read_i32(&mut self) -> thrift::Result<i32> {
        i32::try_from(self.read_zigzag()?)
            .map_err(|_error| invalid_data("compact i32 is out of range"))
    }

    fn read_i64(&mut self) -> thrift::Result<i64> {
        self.read_zigzag()
    }

    fn read_double(&mut self) -> thrift::Result<f64> {
        let bytes: [u8; 8] = self
            .remaining
            .get(..8)
            .ok_or_else(|| end_of_file("double exceeds remaining input"))?
            .try_into()
            .map_err(|_error| invalid_data("double has an invalid width"))?;
        self.remaining = &self.remaining[8..];
        Ok(f64::from_le_bytes(bytes))
    }

    fn read_string(&mut self) -> thrift::Result<String> {
        String::from_utf8(self.read_bytes()?).map_err(Into::into)
    }

    fn read_list_begin(&mut self) -> thrift::Result<TListIdentifier> {
        let (element_type, size) = self.read_collection()?;
        Ok(TListIdentifier::new(element_type, size))
    }

    fn read_list_end(&mut self) -> thrift::Result<()> {
        self.leave()
    }

    fn read_set_begin(&mut self) -> thrift::Result<TSetIdentifier> {
        Err(not_implemented(
            "sets are not valid in supported Parquet metadata",
        ))
    }

    fn read_set_end(&mut self) -> thrift::Result<()> {
        Err(not_implemented(
            "sets are not valid in supported Parquet metadata",
        ))
    }

    fn read_map_begin(&mut self) -> thrift::Result<TMapIdentifier> {
        Err(not_implemented(
            "maps are not valid in supported Parquet metadata",
        ))
    }

    fn read_map_end(&mut self) -> thrift::Result<()> {
        Err(not_implemented(
            "maps are not valid in supported Parquet metadata",
        ))
    }

    fn read_byte(&mut self) -> thrift::Result<u8> {
        let byte = *self
            .remaining
            .first()
            .ok_or_else(|| end_of_file("compact metadata ended unexpectedly"))?;
        self.remaining = &self.remaining[1..];
        Ok(byte)
    }
}

pub(super) fn compact_type(value: u8) -> thrift::Result<TType> {
    match value {
        0 => Ok(TType::Stop),
        1 | 2 => Ok(TType::Bool),
        3 => Ok(TType::I08),
        4 => Ok(TType::I16),
        5 => Ok(TType::I32),
        6 => Ok(TType::I64),
        7 => Ok(TType::Double),
        8 => Ok(TType::String),
        9 => Ok(TType::List),
        10 => Ok(TType::Set),
        11 => Ok(TType::Map),
        12 => Ok(TType::Struct),
        _ => Err(invalid_data("unknown compact field type")),
    }
}

pub(super) fn invalid_data(message: &'static str) -> thrift::Error {
    thrift::Error::Protocol(thrift::ProtocolError {
        kind: thrift::ProtocolErrorKind::InvalidData,
        message: message.to_owned(),
    })
}

pub(super) fn not_implemented(message: &'static str) -> thrift::Error {
    thrift::Error::Protocol(thrift::ProtocolError {
        kind: thrift::ProtocolErrorKind::NotImplemented,
        message: message.to_owned(),
    })
}

pub(super) fn end_of_file(message: &'static str) -> thrift::Error {
    thrift::Error::Transport(thrift::TransportError {
        kind: thrift::TransportErrorKind::EndOfFile,
        message: message.to_owned(),
    })
}
