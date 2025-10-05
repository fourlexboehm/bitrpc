use bitcode::Buffer as BitcodeBuffer;
use compio_buf::bytes::{Bytes, BytesMut};
use std::cell::RefCell;
use std::rc::Rc;

const BITCODE_BUFFER_POOL_MAX_SIZE: usize = 64;
const BODY_POOL_DEFAULT_CAPACITY: usize = 16 * 1024;
const BODY_POOL_MAX_CAPACITY: usize = 256 * 1024;
const BODY_POOL_MAX_SIZE: usize = 64;

#[derive(Clone)]
pub struct BitcodeBufferPool {
    inner: Rc<RefCell<Vec<BitcodeBuffer>>>,
}

impl BitcodeBufferPool {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn take(&self) -> BitcodeBuffer {
        self.inner.borrow_mut().pop().unwrap_or_default()
    }

    pub fn recycle(&self, buffer: BitcodeBuffer) {
        let mut pool = self.inner.borrow_mut();
        if pool.len() < BITCODE_BUFFER_POOL_MAX_SIZE {
            pool.push(buffer);
        }
    }
}

pub struct PooledBitcodeBuffer {
    pool: BitcodeBufferPool,
    inner: Option<BitcodeBuffer>,
}

impl PooledBitcodeBuffer {
    pub fn new(pool: BitcodeBufferPool) -> Self {
        Self {
            inner: Some(pool.take()),
            pool,
        }
    }

    pub fn take_output_vec(&mut self) -> Vec<u8> {
        const OUT_OFFSET: usize =
            std::mem::size_of::<BitcodeBuffer>() - std::mem::size_of::<Vec<u8>>();

        unsafe {
            let buffer = self.inner.as_mut().expect("bitcode buffer missing");
            // Safety: `bitcode::Buffer` stores `Vec<u8>` as its last field.
            // We rely on that layout (stable in 0.6.x) to swap the vector without copying.
            let vec_ptr = (buffer as *mut BitcodeBuffer as *mut u8).add(OUT_OFFSET) as *mut Vec<u8>;
            let vec = std::ptr::read(vec_ptr);
            std::ptr::write(vec_ptr, Vec::new());
            vec
        }
    }

    pub fn restore_output_vec(&mut self, mut vec: Vec<u8>) {
        const OUT_OFFSET: usize =
            std::mem::size_of::<BitcodeBuffer>() - std::mem::size_of::<Vec<u8>>();

        vec.clear();

        unsafe {
            let buffer = self.inner.as_mut().expect("bitcode buffer missing");
            let vec_ptr = (buffer as *mut BitcodeBuffer as *mut u8).add(OUT_OFFSET) as *mut Vec<u8>;
            let old = std::ptr::replace(vec_ptr, vec);
            drop(old);
        }
    }
}

impl std::ops::Deref for PooledBitcodeBuffer {
    type Target = BitcodeBuffer;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("bitcode buffer missing")
    }
}

impl std::ops::DerefMut for PooledBitcodeBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().expect("bitcode buffer missing")
    }
}

impl Drop for PooledBitcodeBuffer {
    fn drop(&mut self) {
        if let Some(buffer) = self.inner.take() {
            self.pool.recycle(buffer);
        }
    }
}

pub struct PooledEncodedBytes {
    buffer: PooledBitcodeBuffer,
    bytes: Option<Bytes>,
}

impl PooledEncodedBytes {
    pub fn from_encoded_buffer(mut buffer: PooledBitcodeBuffer) -> Self {
        let vec = buffer.take_output_vec();
        let bytes = Bytes::from(vec);
        Self {
            buffer,
            bytes: Some(bytes),
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.as_ref().expect("encoded bytes missing").len()
    }

    pub fn bytes(&self) -> Bytes {
        self.bytes.as_ref().expect("encoded bytes missing").clone()
    }
}

impl Drop for PooledEncodedBytes {
    fn drop(&mut self) {
        if let Some(bytes) = self.bytes.take() {
            let mut vec: Vec<u8> = bytes.into();
            vec.clear();
            self.buffer.restore_output_vec(vec);
        }
    }
}

#[derive(Clone)]
pub struct BytesMutPool {
    inner: Rc<RefCell<Vec<BytesMut>>>,
}

impl BytesMutPool {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn take(&self) -> BytesMut {
        self.inner
            .borrow_mut()
            .pop()
            .unwrap_or_else(|| BytesMut::with_capacity(BODY_POOL_DEFAULT_CAPACITY))
    }

    pub fn recycle(&self, mut buf: BytesMut) {
        if buf.capacity() > BODY_POOL_MAX_CAPACITY {
            return;
        }

        buf.clear();

        let mut pool = self.inner.borrow_mut();
        if pool.len() < BODY_POOL_MAX_SIZE {
            pool.push(buf);
        }
    }
}

pub struct PooledBytesMut {
    pool: BytesMutPool,
    inner: Option<BytesMut>,
}

impl PooledBytesMut {
    pub fn new(pool: BytesMutPool) -> Self {
        Self {
            inner: Some(pool.take()),
            pool,
        }
    }
}

impl std::ops::Deref for PooledBytesMut {
    type Target = BytesMut;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("pooled body missing")
    }
}

impl std::ops::DerefMut for PooledBytesMut {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().expect("pooled body missing")
    }
}

impl Drop for PooledBytesMut {
    fn drop(&mut self) {
        if let Some(buf) = self.inner.take() {
            self.pool.recycle(buf);
        }
    }
}
