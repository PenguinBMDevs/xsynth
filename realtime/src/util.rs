// ReadWriteAtomicU64 eliminated.
// Old implementation used UnsafeCell<u64> without any atomic ordering, which
// is undefined behavior when shared across threads.
// All usages replaced with std::sync::atomic::AtomicU64, which provides
// proper memory ordering guarantees without the overhead of a Mutex.
