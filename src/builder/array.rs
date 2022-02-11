//! Array builder.

use crate::binary::{
    ARRAY_SIZE, DATA_TYPE_SIZE, ELEMENT_COUNT_SIZE, MAX_DATA_LENGTH_SIZE, NUMBER_LENGTH_SIZE, VALUE_ENTRY_SIZE,
};
use crate::builder::object::InnerObjectBuilder;
use crate::builder::{BuildResult, BytesWrapper, DEFAULT_SIZE};
use crate::vec::VecExt;
use crate::yason::{Yason, YasonBuf};
use crate::{BuildError, DataType, Number, ObjectRefBuilder};
use decimal_rs::MAX_BINARY_SIZE;

pub(crate) struct InnerArrayBuilder<B: AsMut<Vec<u8>>> {
    bytes_wrapper: BytesWrapper<B>,
    element_count: u16,
    start_pos: usize,
    value_entry_pos: usize,
    value_count: u16,
    depth: usize,
    bytes_init_len: usize,
}

impl<B: AsMut<Vec<u8>>> InnerArrayBuilder<B> {
    #[inline]
    pub(crate) fn try_new(bytes: B, element_count: u16) -> BuildResult<Self> {
        let mut bytes_wrapper = BytesWrapper::new(bytes);
        let bytes = bytes_wrapper.bytes.as_mut();
        let bytes_init_len = bytes.len();

        let size = DATA_TYPE_SIZE + ARRAY_SIZE + ELEMENT_COUNT_SIZE + VALUE_ENTRY_SIZE * element_count as usize;
        bytes.try_reserve(size)?;

        bytes.push_data_type(DataType::Array); // type
        bytes.skip_size(); // size
        let start_pos = bytes.len();
        bytes.push_u16(element_count); // element-count
        let value_entry_pos = bytes.len();
        bytes.skip_value_entry(element_count as usize); // value-entry
        bytes_wrapper.depth += 1;

        Ok(Self {
            depth: bytes_wrapper.depth,
            bytes_wrapper,
            element_count,
            start_pos,
            value_entry_pos,
            value_count: 0,
            bytes_init_len,
        })
    }

    #[inline]
    fn finish(&mut self) -> BuildResult<usize> {
        let bytes = self.bytes_wrapper.bytes.as_mut();

        if self.depth != self.bytes_wrapper.depth {
            return Err(BuildError::InnerUncompletedError);
        }

        if self.value_count != self.element_count {
            return Err(BuildError::InconsistentElementCount {
                expected: self.element_count,
                actual: self.value_count,
            });
        }

        let total_size = bytes.len() - self.start_pos;
        bytes.write_total_size(total_size as i32, self.start_pos - ARRAY_SIZE);
        self.bytes_wrapper.depth -= 1;

        Ok(self.bytes_init_len)
    }

    #[inline]
    fn push_value<F>(&mut self, data_type: DataType, f: F) -> BuildResult<()>
    where
        F: FnOnce(&mut Vec<u8>, u32, usize) -> BuildResult<()>,
    {
        if self.depth != self.bytes_wrapper.depth {
            return Err(BuildError::InnerUncompletedError);
        }

        let bytes = self.bytes_wrapper.bytes.as_mut();
        bytes.write_data_type_by_pos(data_type, self.value_entry_pos);
        let offset = bytes.len() - self.start_pos;

        f(bytes, offset as u32, self.value_entry_pos)?;

        self.value_entry_pos += VALUE_ENTRY_SIZE;
        self.value_count += 1;
        Ok(())
    }

    #[inline]
    fn push_object(&mut self, element_count: u16, key_sorted: bool) -> BuildResult<InnerObjectBuilder<&mut Vec<u8>>> {
        let f = |bytes: &mut Vec<u8>, offset: u32, value_entry_pos: usize| {
            bytes.write_offset(offset, value_entry_pos + DATA_TYPE_SIZE);
            Ok(())
        };
        self.push_value(DataType::Object, f)?;

        let bytes = self.bytes_wrapper.bytes.as_mut();
        InnerObjectBuilder::try_new(bytes, element_count, key_sorted)
    }

    #[inline]
    fn push_array(&mut self, element_count: u16) -> BuildResult<InnerArrayBuilder<&mut Vec<u8>>> {
        let f = |bytes: &mut Vec<u8>, offset: u32, value_entry_pos: usize| {
            bytes.write_offset(offset, value_entry_pos + DATA_TYPE_SIZE);
            Ok(())
        };
        self.push_value(DataType::Array, f)?;

        let bytes = self.bytes_wrapper.bytes.as_mut();
        InnerArrayBuilder::try_new(bytes, element_count)
    }

    #[inline]
    fn push_string(&mut self, value: &str) -> BuildResult<()> {
        let size = MAX_DATA_LENGTH_SIZE + value.len();
        let f = |bytes: &mut Vec<u8>, offset: u32, value_entry_pos: usize| {
            bytes.write_offset(offset, value_entry_pos + DATA_TYPE_SIZE);
            bytes.try_reserve(size)?;
            bytes.push_string(value)?;
            Ok(())
        };
        self.push_value(DataType::String, f)
    }

    #[inline]
    fn push_number(&mut self, value: Number) -> BuildResult<()> {
        let size = MAX_BINARY_SIZE + NUMBER_LENGTH_SIZE;
        let f = |bytes: &mut Vec<u8>, offset: u32, value_entry_pos: usize| {
            bytes.write_offset(offset, value_entry_pos + DATA_TYPE_SIZE);
            bytes.try_reserve(size)?;
            bytes.push_number(value);
            Ok(())
        };
        self.push_value(DataType::Number, f)
    }

    #[inline]
    fn push_bool(&mut self, value: bool) -> BuildResult<()> {
        // bool can be inlined
        let f = |bytes: &mut Vec<u8>, _offset: u32, value_entry_pos: usize| {
            bytes.write_offset(value as u32, value_entry_pos + DATA_TYPE_SIZE);
            Ok(())
        };
        self.push_value(DataType::Bool, f)
    }

    #[inline]
    fn push_null(&mut self) -> BuildResult<()> {
        // null can be inlined
        self.push_value(DataType::Null, |_, _, _| Ok(()))
    }
}

/// Builder for encoding an array.
#[repr(transparent)]
pub struct ArrayBuilder(InnerArrayBuilder<Vec<u8>>);

impl ArrayBuilder {
    /// Creates `ArrayBuilder` with specified element count.
    #[inline]
    pub fn try_new(element_count: u16) -> BuildResult<Self> {
        let bytes = Vec::try_with_capacity(DEFAULT_SIZE)?;
        let builder = InnerArrayBuilder::try_new(bytes, element_count)?;
        Ok(Self(builder))
    }

    /// Finishes building the array.
    #[inline]
    pub fn finish(mut self) -> BuildResult<YasonBuf> {
        self.0.finish()?;
        Ok(unsafe { YasonBuf::new_unchecked(self.0.bytes_wrapper.bytes) })
    }

    /// Pushes an embedded object with specified element count and a flag which indicates whether the embedded object is sorted by key.
    #[inline]
    pub fn push_object(&mut self, element_count: u16, key_sorted: bool) -> BuildResult<ObjectRefBuilder> {
        let obj_builder = self.0.push_object(element_count, key_sorted)?;
        Ok(ObjectRefBuilder(obj_builder))
    }

    /// Pushes an embedded array with specified element count.
    #[inline]
    pub fn push_array(&mut self, element_count: u16) -> BuildResult<ArrayRefBuilder> {
        let array_builder = self.0.push_array(element_count)?;
        Ok(ArrayRefBuilder(array_builder))
    }

    /// Pushes a string value.
    #[inline]
    pub fn push_string<Val: AsRef<str>>(&mut self, value: Val) -> BuildResult<&mut Self> {
        let value = value.as_ref();
        self.0.push_string(value)?;
        Ok(self)
    }

    /// Pushes a number value.
    #[inline]
    pub fn push_number(&mut self, value: Number) -> BuildResult<&mut Self> {
        self.0.push_number(value)?;
        Ok(self)
    }

    /// Pushes a bool value.
    #[inline]
    pub fn push_bool(&mut self, value: bool) -> BuildResult<&mut Self> {
        self.0.push_bool(value)?;
        Ok(self)
    }

    /// Pushes a null value.
    #[inline]
    pub fn push_null(&mut self) -> BuildResult<&mut Self> {
        self.0.push_null()?;
        Ok(self)
    }
}

/// Builder for encoding an array.
#[repr(transparent)]
pub struct ArrayRefBuilder<'a>(pub(crate) InnerArrayBuilder<&'a mut Vec<u8>>);

impl<'a> ArrayRefBuilder<'a> {
    /// Creates `ArrayRefBuilder` with specified element count.
    #[inline]
    pub fn try_new(bytes: &'a mut Vec<u8>, element_count: u16) -> BuildResult<Self> {
        let array_builder = InnerArrayBuilder::try_new(bytes, element_count)?;
        Ok(Self(array_builder))
    }

    /// Finishes building the array.
    #[inline]
    pub fn finish(mut self) -> BuildResult<&'a Yason> {
        let bytes_init_len = self.0.finish()?;
        let bytes = self.0.bytes_wrapper.bytes;
        Ok(unsafe { Yason::new_unchecked(&bytes[bytes_init_len..]) })
    }

    /// Creates an `ObjectRefBuilder` for the embedded object with specified element count.
    #[inline]
    pub fn push_object(&mut self, element_count: u16, key_sorted: bool) -> BuildResult<ObjectRefBuilder> {
        let obj_builder = self.0.push_object(element_count, key_sorted)?;
        Ok(ObjectRefBuilder(obj_builder))
    }

    /// Creates an embedded array with specified element count.
    #[inline]
    pub fn push_array(&mut self, element_count: u16) -> BuildResult<ArrayRefBuilder> {
        let array_builder = self.0.push_array(element_count)?;
        Ok(ArrayRefBuilder(array_builder))
    }

    /// Pushes a string value.
    #[inline]
    pub fn push_string<Val: AsRef<str>>(&mut self, value: Val) -> BuildResult<&mut Self> {
        let value = value.as_ref();
        self.0.push_string(value)?;
        Ok(self)
    }

    /// Pushes a number value.
    #[inline]
    pub fn push_number(&mut self, value: Number) -> BuildResult<&mut Self> {
        self.0.push_number(value)?;
        Ok(self)
    }

    /// Pushes a bool value.
    #[inline]
    pub fn push_bool(&mut self, value: bool) -> BuildResult<&mut Self> {
        self.0.push_bool(value)?;
        Ok(self)
    }

    /// Pushes a null value.
    #[inline]
    pub fn push_null(&mut self) -> BuildResult<&mut Self> {
        self.0.push_null()?;
        Ok(self)
    }
}
