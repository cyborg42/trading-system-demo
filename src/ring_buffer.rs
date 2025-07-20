//! A lock-free ring buffer for high-performance message passing between threads.
//!
//! This ring buffer implements a single-producer, multiple-consumer pattern
//! with zero-copy reads and efficient memory usage. It uses seqlock semantics
//! to ensure thread safety without traditional locks.
//!
//! Seqlock implementation is based on the following blog post:
//! https://pitdicker.github.io/Writing-a-seqlock-in-Rust/

use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr::{self, drop_in_place},
    sync::atomic::{AtomicUsize, Ordering, fence},
};

/// A slot in a channel that holds a single message with version tracking.
///
/// Each slot uses a seqlock pattern to ensure thread-safe access during concurrent
/// read/write operations. The version field tracks the state of the slot:
/// - Even version: slot is ready for reading
/// - Odd version: slot is being written to
struct Slot<T> {
    /// The current seqlock version.
    ///
    /// This atomic counter tracks the state of the slot:
    /// - Even values indicate the slot is ready for reading
    /// - Odd values indicate the slot is currently being written to
    version: AtomicUsize,
    /// The message stored in this slot.
    ///
    /// Uses `UnsafeCell` and `MaybeUninit` to allow for zero-copy reads
    /// and safe concurrent access patterns.
    msg: UnsafeCell<MaybeUninit<T>>,
}

unsafe impl<T> Send for Slot<T> {}
unsafe impl<T> Sync for Slot<T> {}

/// A lock-free ring buffer for high-performance message passing between threads.
///
/// This ring buffer implements a single-producer, multiple-consumer pattern
/// with zero-copy reads and efficient memory usage. It uses seqlock semantics
/// to ensure thread safety without traditional locks.
///
/// # Features
///
/// - **Lock-free**: No mutexes or locks required for thread safety
/// - **Zero-copy reads**: Messages can be read without copying data
/// - **Overflow handling**: Detects and reports message loss due to buffer overflow
/// - **Multiple consumers**: Supports multiple subscribers reading from the same buffer
/// - **High performance**: Optimized for low-latency trading systems
///
/// # Thread Safety
///
/// - **Single producer**: Only one thread should write to the buffer
/// - **Multiple consumers**: Multiple threads can read from the buffer safely
/// - **No blocking**: Reads and writes never block or wait
///
/// # Example
///
/// ```rust
/// use trading_system_demo::ring_buffer::RingBuffer;
///
/// let mut buffer = RingBuffer::new(1000);
/// let (mut publisher, mut subscriber) = buffer.split();
///
/// // Producer thread
/// publisher.write("Hello, World!");
///
/// // Consumer thread
/// if let Some((message, lost_count)) = subscriber.read() {
///     println!("Received: {}, Lost: {}", message, lost_count);
/// }
/// ```
pub struct RingBuffer<T> {
    /// The current write position in the buffer.
    ///
    /// This index points to the next slot where a message will be written.
    /// It wraps around when it reaches the buffer capacity.
    writer_idx: usize,
    /// The underlying buffer containing message slots.
    ///
    /// Each slot contains a version counter and the actual message data.
    buffer: Box<[Slot<T>]>,
    /// The total capacity of the ring buffer.
    ///
    /// This determines how many messages can be stored before overflow occurs.
    cap: usize,
}

impl<T> RingBuffer<T> {
    /// Creates a new ring buffer with the specified capacity.
    ///
    /// # Arguments
    ///
    /// * `cap` - The maximum number of messages the buffer can hold
    ///
    /// # Returns
    ///
    /// A new `RingBuffer` instance with the specified capacity.
    ///
    /// # Capacity Adjustment
    ///
    /// The provided capacity will be automatically adjusted to the next power of 2
    /// (e.g., 1000 becomes 1024, 1500 becomes 2048). This optimization enables
    /// efficient bitwise operations for index wrapping:
    ///
    /// - **Index wrapping**: Instead of using modulo (`%`), we use bitwise AND (`&`)
    ///   with `(cap - 1)` to wrap indices around the buffer
    /// - **Performance**: Bitwise AND is significantly faster than modulo operation
    /// - **Memory efficiency**: Powers of 2 allow for optimal memory alignment
    ///
    /// # Example
    ///
    /// ```rust
    /// use trading_system_demo::ring_buffer::RingBuffer;
    ///
    /// // Capacity 1000 will be adjusted to 1024 (2^10)
    /// let buffer = RingBuffer::<String>::new(1000);
    ///
    /// // Capacity 1500 will be adjusted to 2048 (2^11)
    /// let buffer2 = RingBuffer::<String>::new(1500);
    /// ```
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(2).next_power_of_two();
        let buffer: Box<[Slot<T>]> = (0..cap)
            .map(|_| Slot {
                version: AtomicUsize::new(0),
                msg: UnsafeCell::new(MaybeUninit::uninit()),
            })
            .collect();
        Self {
            writer_idx: 0,
            buffer,
            cap,
        }
    }

    /// Returns the capacity of the ring buffer.
    pub fn get_cap(&self) -> usize {
        self.cap
    }

    /// Splits the ring buffer into a publisher and subscriber pair.
    ///
    /// The publisher is used to write messages to the buffer, while the subscriber
    /// is used to read messages from the buffer. Multiple subscribers can be created
    /// by cloning the returned subscriber.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - `Publisher`: Used to write messages to the buffer
    /// - `Subscriber`: Used to read messages from the buffer
    ///
    /// # Example
    ///
    /// ```rust
    /// use trading_system_demo::ring_buffer::RingBuffer;
    ///
    /// let mut buffer = RingBuffer::<i32>::new(100);
    /// let (mut publisher, mut subscriber) = buffer.split();
    ///
    /// // Create multiple subscribers
    /// let mut subscriber2 = subscriber.clone();
    /// ```
    pub fn split(&mut self) -> (Publisher<'_, T>, Subscriber<'_, T>) {
        let publisher = Publisher {
            writer_idx: &mut self.writer_idx,
            buffer: &self.buffer,
            cap_mask: self.cap - 1,
        };
        let subscriber = Subscriber {
            version: 2,
            reader_idx: 0,
            buffer: &self.buffer,
            cap_mask: self.cap - 1,
            cap_zeros_sub_one: self.cap.trailing_zeros() as usize - 1,
        };
        (publisher, subscriber)
    }
}

impl<T> Drop for RingBuffer<T> {
    fn drop(&mut self) {
        if !std::mem::needs_drop::<T>() {
            return;
        }
        for slot in self.buffer.iter() {
            if slot.version.load(Ordering::Relaxed) == 0 {
                break;
            }
            unsafe {
                drop_in_place(slot.msg.get().cast::<T>());
            }
        }
    }
}

/// A publisher that writes messages to a ring buffer.
///
/// The publisher is responsible for writing messages to the ring buffer.
/// Only one publisher should exist per ring buffer to maintain thread safety.
///
/// # Thread Safety
///
/// - **Single-threaded**: Only one thread should use a publisher instance
/// - **Non-blocking**: Writes never block or wait
/// - **Overflow handling**: When the buffer is full, old messages are overwritten
///
/// # Example
///
/// ```rust
/// use trading_system_demo::ring_buffer::RingBuffer;
///
/// let mut buffer = RingBuffer::new(100);
/// let (mut publisher, _) = buffer.split();
///
/// publisher.write("Message 1");
/// publisher.write("Message 2");
/// ```
pub struct Publisher<'a, T> {
    /// Reference to the writer index in the parent ring buffer.
    ///
    /// This tracks the current write position and is updated after each write.
    writer_idx: &'a mut usize,
    /// Reference to the buffer slots in the parent ring buffer.
    ///
    /// Contains all the message slots that can be written to.
    buffer: &'a [Slot<T>],
    /// The capacity mask of the ring buffer, which is (cap - 1).
    ///
    /// Used to wrap the writer index when it reaches the end of the buffer.
    /// Since the capacity is always a power of 2, this mask contains all 1s
    /// in the lower bits, enabling fast bitwise AND operations for modulo.
    /// For example, if cap = 1024, then cap_mask = 1023 (binary: 1111111111).
    cap_mask: usize,
}

impl<'a, T> Publisher<'a, T> {
    /// Writes a message to the ring buffer.
    ///
    /// This method writes a message to the current write position and advances
    /// the write index. If the buffer is full, the oldest message will be overwritten.
    pub fn write(&mut self, msg: T)
    where
        T: Copy,
    {
        let slot = unsafe { &self.buffer.get_unchecked(*self.writer_idx) };
        let version = slot.version.fetch_add(1, Ordering::Acquire);
        // Only one thread can write to the buffer at a time, so we don't need to check for version
        debug_assert!(version & 1 == 0);
        unsafe {
            ptr::write_volatile(slot.msg.get().cast::<T>(), msg);
        }
        slot.version.fetch_add(1, Ordering::Release);
        *self.writer_idx = (*self.writer_idx + 1) & self.cap_mask;
    }

    /// Write a message to the ring buffer and drop the previous message if it exists.
    ///
    /// This method writes a message to the current write position and advances
    /// the write index. If the buffer is full, the oldest message will be overwritten.
    pub fn write_clean(&mut self, msg: T) {
        let slot = unsafe { self.buffer.get_unchecked(*self.writer_idx) };
        let version = slot.version.fetch_add(1, Ordering::Acquire);
        // Only one thread can write to the buffer at a time, so we don't need to check for version
        debug_assert!(version & 1 == 0);
        if version != 0 {
            unsafe {
                drop_in_place(slot.msg.get().cast::<T>());
            }
        }
        unsafe {
            ptr::write_volatile(slot.msg.get().cast::<T>(), msg);
        }
        slot.version.fetch_add(1, Ordering::Release);
        *self.writer_idx = (*self.writer_idx + 1) & self.cap_mask;
    }
}

/// A subscriber that reads messages from a ring buffer.
///
/// The subscriber is responsible for reading messages from the ring buffer.
/// Multiple subscribers can exist for the same ring buffer, allowing for
/// multiple consumers to process messages independently.
///
/// # Thread Safety
///
/// - **Multi-threaded**: Multiple threads can use different subscriber instances
/// - **Non-blocking**: Reads never block or wait
/// - **Zero-copy**: Messages can be read without copying data
///
/// # Example
///
/// ```rust
/// use trading_system_demo::ring_buffer::RingBuffer;
///
/// let mut buffer = RingBuffer::new(100);
/// let (mut publisher, mut subscriber) = buffer.split();
///
/// publisher.write("Hello");
///
/// if let Some((message, lost_count)) = subscriber.read() {
///     println!("Received: {}", message);
/// }
/// ```
#[derive(Clone)]
pub struct Subscriber<'a, T> {
    /// The expected version number for the next message to read.
    ///
    /// This tracks the seqlock version that the subscriber expects to see
    /// for the next message. It's used to detect message loss and ensure
    /// proper synchronization.
    version: usize,
    /// The current read position in the buffer.
    ///
    /// This index points to the next slot to read from. It wraps around
    /// when it reaches the buffer capacity.
    reader_idx: usize,
    /// Reference to the buffer slots in the parent ring buffer.
    ///
    /// Contains all the message slots that can be read from.
    buffer: &'a [Slot<T>],
    /// The capacity mask of the ring buffer, which is (cap - 1).
    ///
    /// Used to wrap the reader index when it reaches the end of the buffer.
    /// Since the capacity is always a power of 2, this mask contains all 1s
    /// in the lower bits, enabling fast bitwise AND operations for modulo.
    /// For example, if cap = 1024, then cap_mask = 1023 (binary: 1111111111).
    cap_mask: usize,
    /// The number of zeros in the capacity, minus 1.
    ///
    /// Used to calculate the lost count.
    cap_zeros_sub_one: usize,
}

impl<'a, T> Subscriber<'a, T> {
    /// Read a message from the ring buffer using bitwise copy.
    ///
    /// This method reads a message from the current read position and advances
    /// the read index. It returns both the message and information about any
    /// messages that were lost due to buffer overflow.
    ///
    /// # Return Value
    ///
    /// Returns `Some((message, lost_count))` where:
    /// - `message`: The message read from the buffer
    /// - `lost_count`: The number of messages that were lost due to buffer overflow
    ///
    /// Returns `None` if no new messages are available.
    ///
    /// # Safety
    ///
    /// This method is unsafe and should only be used for types that can be safely
    /// bitwise copied without implementing `Copy`. This includes:
    /// - Custom types that are designed for bitwise copying
    /// - Types that don't own heap-allocated data
    ///
    /// # ⚠️ Important Warnings
    ///
    /// **DO NOT use this method with types that own heap-allocated data** such as:
    /// - `String` - Will cause double free of the string's buffer
    /// - `Vec<T>` - Will cause double free of the vector's buffer
    /// - `Box<T>` - Will cause double free of the boxed data
    /// - Any type containing `String`, `Vec`, `Box`, etc.
    ///
    /// For types that own heap data, use `read_clone()` instead, which safely
    /// clones the data and avoids double free issues.
    ///
    /// # Example
    ///
    /// ```rust
    /// use trading_system_demo::ring_buffer::RingBuffer;
    ///
    /// // ✅ Safe: Custom type designed for bitwise copy
    /// #[repr(C)]
    /// #[derive(Copy, Clone)]
    /// struct SafeType {
    ///     a: u32,
    ///     b: u64,
    /// }
    ///
    /// let mut buffer = RingBuffer::new(10);
    /// let (mut publisher, mut subscriber) = buffer.split();
    /// publisher.write(SafeType { a: 1, b: 2 });
    ///
    /// // Safe to use read_copy for SafeType
    /// if let Some((msg, lost)) = unsafe { subscriber.read_copy() } {
    ///     println!("Received: a={}, b={}", msg.a, msg.b);
    /// }
    ///
    /// // ❌ DANGEROUS: Never do this with String
    /// // let mut buffer = RingBuffer::new(10);
    /// // let (mut publisher, mut subscriber) = buffer.split();
    /// // publisher.write("hello".to_string());
    /// // let result = unsafe { subscriber.read_copy() }; // DOUBLE FREE!
    /// ```
    pub unsafe fn read_copy(&mut self) -> Option<(T, usize)> {
        let slot = unsafe { self.buffer.get_unchecked(self.reader_idx) };
        loop {
            let version = slot.version.load(Ordering::Acquire);
            if version & 1 != 0 {
                // Message is being written
                std::hint::spin_loop();
                continue;
            }
            if version < self.version {
                // No new messages
                return None;
            }
            let msg = unsafe { ptr::read_volatile(slot.msg.get().cast::<T>()) };
            fence(Ordering::Acquire);
            let new_version = slot.version.load(Ordering::Relaxed);
            if version != new_version {
                // Message is being written
                std::hint::spin_loop();
                continue;
            }
            let lost_count = (version - self.version) << self.cap_zeros_sub_one;
            self.version = version;
            self.reader_idx = (self.reader_idx + 1) & self.cap_mask;
            self.version += ((self.reader_idx == 0) as usize) << 1;
            return Some((msg, lost_count));
        }
    }

    /// Read a message from the ring buffer.
    ///
    /// This method reads a message from the current read position and advances
    /// the read index. It returns both the message and information about any
    /// messages that were lost due to buffer overflow.
    ///
    /// # Return Value
    ///
    /// Returns `Some((message, lost_count))` where:
    /// - `message`: The message read from the buffer
    /// - `lost_count`: The number of messages that were lost due to buffer overflow
    ///
    /// Returns `None` if no new messages are available.
    ///
    /// # Performance
    ///
    /// This method uses zero-copy reads for optimal performance. The message
    /// is read directly from the buffer without any copying, making it very
    /// efficient for large messages.
    /// ```
    pub fn read(&mut self) -> Option<(T, usize)>
    where
        T: Copy,
    {
        unsafe { self.read_copy() }
    }

    /// Read a message from the ring buffer.
    ///
    /// This method reads a message from the current read position and advances
    /// the read index. It returns both the message and information about any
    /// messages that were lost due to buffer overflow.
    ///
    /// # Return Value
    ///
    /// Returns `Some((message, lost_count))` where:
    /// - `message`: The message read from the buffer
    /// - `lost_count`: The number of messages that were lost due to buffer overflow
    ///
    /// Returns `None` if no new messages are available.
    ///
    /// # Performance
    ///
    /// This method clones the message when reading, which may have performance
    /// implications for large or complex types. For better performance with
    /// simple types, consider using `read()` if the type implements `Copy`.
    pub fn read_clone(&mut self) -> Option<(T, usize)>
    where
        T: Clone,
    {
        unsafe {
            self.read_copy().map(|(msg, lost_count)| {
                let new_msg = msg.clone();
                std::mem::forget(msg);
                (new_msg, lost_count)
            })
        }
    }

    /// Read a message from the ring buffer with spinning.
    ///
    /// This method continuously spins until a message becomes available.
    /// It's useful for high-performance scenarios where you want to wait
    /// for messages without blocking.
    ///
    /// # Return Value
    ///
    /// Returns `(message, lost_count)` where:
    /// - `message`: The message read from the buffer
    /// - `lost_count`: The number of messages that were lost due to buffer overflow
    ///
    /// # Thread Safety
    ///
    /// This method is thread-safe and can be called concurrently from multiple
    /// threads using different subscriber instances.
    ///
    /// # Performance Considerations
    ///
    /// This method will consume CPU cycles while waiting for messages.
    /// Use this only when you expect messages to arrive quickly and
    /// want to minimize latency.
    ///
    /// # Example
    ///
    /// ```rust
    /// use trading_system_demo::ring_buffer::RingBuffer;
    ///
    /// let mut buffer = RingBuffer::new(100);
    /// let (mut publisher, mut subscriber) = buffer.split();
    ///
    /// // Write a message
    /// publisher.write("Hello");
    ///
    /// // Wait for message with spinning
    /// let (message, lost_count) = subscriber.read_spinning();
    /// println!("Received: {}, Lost: {}", message, lost_count);
    /// ```
    pub fn read_spinning(&mut self) -> (T, usize)
    where
        T: Copy,
    {
        loop {
            if let Some(result) = self.read() {
                return result;
            }
            std::hint::spin_loop();
        }
    }
}

/// An iterator that continuously reads messages from a ring buffer subscriber.
///
/// This iterator will spin when no messages are available, making it suitable
/// for high-performance scenarios where you want to process messages as they arrive
/// without blocking.
///
/// # Performance Considerations
///
/// This iterator will consume CPU cycles while waiting for messages.
/// Use this only when you expect a continuous stream of messages and
/// want to minimize latency.
///
/// # Example
///
/// ```rust
/// use trading_system_demo::ring_buffer::RingBuffer;
///
/// let mut buffer = RingBuffer::new(100);
/// let (mut publisher, mut subscriber) = buffer.split();
///
/// // Write messages
/// for i in 0..5 {
///     publisher.write(i);
/// }
///
/// // Process messages as they arrive
/// for (message, lost_count) in subscriber.spinning_iter() {
///     println!("Received: {}, Lost: {}", message, lost_count);
///     if message == 4 {
///         break; // Exit after receiving the last message
///     }
/// }
/// ```
pub struct SpinningIterator<'a, 'b, T> {
    subscriber: &'b mut Subscriber<'a, T>,
}

impl<'a, 'b, T> Iterator for SpinningIterator<'a, 'b, T>
where
    T: Copy,
{
    type Item = (T, usize);

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.subscriber.read_spinning())
    }
}

impl<'a, T> Subscriber<'a, T> {
    /// Returns a spinning iterator that continuously reads messages.
    ///
    /// This iterator will spin when no messages are available, making it suitable
    /// for high-performance scenarios where you want to process messages as they arrive
    /// without blocking.
    ///
    /// # Performance Considerations
    ///
    /// This iterator will consume CPU cycles while waiting for messages.
    /// Use this only when you expect a continuous stream of messages and
    /// want to minimize latency.
    ///
    /// # Example
    ///
    /// ```rust
    /// use trading_system_demo::ring_buffer::RingBuffer;
    ///
    /// let mut buffer = RingBuffer::new(100);
    /// let (mut publisher, mut subscriber) = buffer.split();
    ///
    /// // Write messages
    /// for i in 0..3 {
    ///     publisher.write(i);
    /// }
    ///
    /// // Process messages as they arrive
    /// for (message, lost_count) in subscriber.spinning_iter() {
    ///     println!("Received: {}, Lost: {}", message, lost_count);
    ///     if message == 2 {
    ///         break; // Exit after receiving the last message
    ///     }
    /// }
    /// ```
    pub fn spinning_iter<'b>(&'b mut self) -> SpinningIterator<'a, 'b, T> {
        SpinningIterator { subscriber: self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_creation() {
        let rb = RingBuffer::<i32>::new(4);
        assert_eq!(rb.cap, 4);
        assert_eq!(rb.writer_idx, 0);
        assert_eq!(rb.buffer.len(), 4);
    }

    #[test]
    fn test_capacity_adjustment_to_power_of_two() {
        // Test that capacities are adjusted to the next power of 2
        let rb1 = RingBuffer::<i32>::new(1000);
        assert_eq!(rb1.get_cap(), 1024); // 2^10

        let rb2 = RingBuffer::<i32>::new(1500);
        assert_eq!(rb2.get_cap(), 2048); // 2^11

        let rb3 = RingBuffer::<i32>::new(3);
        assert_eq!(rb3.get_cap(), 4); // 2^2

        let rb4 = RingBuffer::<i32>::new(1024);
        assert_eq!(rb4.get_cap(), 1024); // Already a power of 2
    }

    #[test]
    fn test_basic_write_read() {
        let mut rb = RingBuffer::<i32>::new(4);
        let (mut publisher, mut subscriber) = rb.split();

        // Write a message
        publisher.write(42);

        // Read the message
        let result = subscriber.read_clone();
        assert!(result.is_some());
        let (msg, lost) = result.unwrap();
        assert_eq!(msg, 42);
        assert_eq!(lost, 0);
    }

    #[test]
    fn test_multiple_writes_reads() {
        let mut rb = RingBuffer::<i32>::new(4);
        let (mut publisher, mut subscriber) = rb.split();

        // Write multiple messages
        for i in 0..3 {
            publisher.write(i);
        }

        // Read all messages
        for i in 0..3 {
            let result = subscriber.read_clone();
            assert!(result.is_some());
            let (msg, lost) = result.unwrap();
            assert_eq!(msg, i);
            assert_eq!(lost, 0);
        }

        // No more messages
        assert!(subscriber.read_clone().is_none());
    }

    #[test]
    fn test_ring_buffer_wraparound() {
        let mut rb = RingBuffer::<i32>::new(2);
        let (mut publisher, mut subscriber) = rb.split();

        // Fill the buffer
        publisher.write(1);
        publisher.write(2);

        // Read first message
        let result = subscriber.read_clone();
        assert!(result.is_some());
        let (msg, lost) = result.unwrap();
        assert_eq!(msg, 1);
        assert_eq!(lost, 0);

        // Write another message (should overwrite)
        publisher.write(3);

        // Read remaining messages
        let result = subscriber.read_clone();
        assert!(result.is_some());
        let (msg, lost) = result.unwrap();
        assert_eq!(msg, 2);
        assert_eq!(lost, 0);

        let result = subscriber.read_clone();
        assert!(result.is_some());
        let (msg, lost) = result.unwrap();
        assert_eq!(msg, 3);
        assert_eq!(lost, 0);
    }

    #[test]
    fn test_message_loss_detection() {
        let mut rb = RingBuffer::<i32>::new(2);
        let (mut publisher, mut subscriber) = rb.split();

        // Fill buffer and read one message
        publisher.write(1);
        publisher.write(2);
        let _ = subscriber.read_clone(); // Read 1

        // Write many more messages to cause overflow
        for i in 3..10 {
            publisher.write(i);
        }

        // Read remaining messages
        let result = subscriber.read_clone();
        assert!(result.is_some());
        let (msg, lost) = result.unwrap();
        assert_eq!(msg, 8);
        assert_eq!(lost, 6);
    }

    #[test]
    fn test_sequential_rapid_writes() {
        let mut rb = RingBuffer::<i32>::new(10);
        let (mut publisher, mut subscriber) = rb.split();

        // Rapid writes in sequence
        for i in 0..100 {
            publisher.write(i);
        }

        // Read all available messages
        let mut received_count = 0;
        let mut last_received = -1;
        let mut total_lost = 0;

        while let Some((msg, lost)) = subscriber.read_clone() {
            received_count += 1;
            total_lost += lost;
            last_received = msg;
        }

        // Should have received some messages (exact count depends on buffer size)
        assert!(received_count > 0);
        assert!(last_received >= 0);
        println!(
            "Received {} messages, lost {} messages",
            received_count, total_lost
        );
    }

    #[test]
    fn test_subscriber_clone() {
        let mut rb = RingBuffer::<i32>::new(4);
        let (mut publisher, mut subscriber) = rb.split();

        // Clone the subscriber
        let mut subscriber2 = subscriber.clone();

        // Write a message
        publisher.write(42);

        // Both subscribers should be able to read
        let result1 = subscriber.read_clone();
        let result2 = subscriber2.read_clone();

        assert!(result1.is_some());
        assert!(result2.is_some());
        assert_eq!(result1.unwrap().0, 42);
        assert_eq!(result2.unwrap().0, 42);
    }

    #[test]
    fn test_empty_buffer_read() {
        let mut rb = RingBuffer::<i32>::new(4);
        let (_, mut subscriber) = rb.split();

        // Try to read from empty buffer
        assert!(subscriber.read_clone().is_none());
    }

    #[test]
    fn test_string_messages() {
        let mut rb = RingBuffer::<String>::new(3);
        let (mut publisher, mut subscriber) = rb.split();

        // Write string messages
        publisher.write_clean("hello".to_string());
        publisher.write_clean("world".to_string());

        // Read string messages
        let result1 = subscriber.read_clone();
        let result2 = subscriber.read_clone();

        assert!(result1.is_some());
        assert!(result2.is_some());
        assert_eq!(result1.unwrap().0, "hello");
        assert_eq!(result2.unwrap().0, "world");
    }

    #[test]
    fn test_custom_struct() {
        #[derive(Clone, Debug, PartialEq)]
        struct TestStruct {
            id: u32,
            data: String,
        }

        let mut rb = RingBuffer::<TestStruct>::new(2);
        let (mut publisher, mut subscriber) = rb.split();

        let test_data = TestStruct {
            id: 1,
            data: "test".to_string(),
        };

        publisher.write_clean(test_data.clone());

        let result = subscriber.read_clone();
        assert!(result.is_some());
        let (msg, _) = result.unwrap();
        assert_eq!(msg, test_data);
    }

    #[test]
    fn test_large_capacity() {
        let mut rb = RingBuffer::<i32>::new(1000);
        let (mut publisher, mut subscriber) = rb.split();

        // Write many messages
        for i in 0..1000 {
            publisher.write(i);
        }

        // Read all messages
        for i in 0..1000 {
            let result = subscriber.read_clone();
            assert!(result.is_some());
            let (msg, lost) = result.unwrap();
            assert_eq!(msg, i);
            assert_eq!(lost, 0);
        }
    }

    #[test]
    fn test_rapid_writes() {
        let mut rb = RingBuffer::<i32>::new(5);
        let (mut publisher, mut subscriber) = rb.split();

        // Rapid writes
        for i in 0..100 {
            publisher.write(i);
        }

        // Read all available messages
        let mut count = 0;
        while let Some((_msg, _lost)) = subscriber.read_clone() {
            count += 1;
            // lost is usize, so it's always >= 0
        }

        // Should have read some messages (exact count depends on timing)
        assert!(count > 0);
    }
}
