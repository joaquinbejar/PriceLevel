use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// # UuidGenerator
///
/// A utility for generating sequential UUIDs within a namespace.
///
/// This struct provides a thread-safe way to generate UUIDs using the UUID v5 algorithm,
/// which creates name-based UUIDs. Each generated UUID is unique within the given namespace
/// and derived from an incrementing counter.
///
/// ## Example
///
/// ```
/// use uuid::Uuid;
/// use pricelevel::UuidGenerator;
///
/// let namespace = Uuid::new_v4(); // Create a random namespace
/// let generator = UuidGenerator::new(namespace);
///
/// let id1 = generator.next(); // Generate first UUID
/// let id2 = generator.next(); // Generate second UUID
/// ```
///
/// This is useful for applications that need deterministic but unique identifiers
/// within a specific namespace context.
#[derive(Debug, Serialize, Deserialize)]
pub struct UuidGenerator {
    namespace: Uuid,
    counter: AtomicU64,
}

/// A generator for creating sequential UUIDs based on a namespace.
///
/// This struct provides functionality to generate deterministic UUIDs in sequence
/// by combining a namespace UUID with an incrementing counter value. Each generated
/// UUID is created using the UUID v5 algorithm (SHA-1 hash-based).
///
/// # Examples
///
/// ```
/// use uuid::Uuid;
/// use pricelevel::UuidGenerator;
///
/// let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
/// let generator = UuidGenerator::new(namespace);
///
/// let id1 = generator.next(); // First UUID
/// let id2 = generator.next(); // Second UUID (different from first)
/// ```
impl UuidGenerator {
    /// Creates a new `UuidGenerator` with the specified namespace.
    ///
    /// The namespace is used as a base for all generated UUIDs.
    ///
    /// # Arguments
    ///
    /// * `namespace` - The UUID to use as the namespace for generating v5 UUIDs
    ///
    /// # Returns
    ///
    /// A new `UuidGenerator` instance initialized with the provided namespace and a counter set to 0.
    pub fn new(namespace: Uuid) -> Self {
        Self {
            namespace,
            counter: AtomicU64::new(0),
        }
    }

    /// Generates the next UUID in sequence.
    ///
    /// This method atomically increments an internal counter and uses its string representation
    /// as the name to generate a UUID v5 combined with the namespace.
    ///
    /// # Returns
    ///
    /// A new UUID that is deterministically derived from the namespace and the current counter value.
    pub fn next(&self) -> Uuid {
        let counter = self.counter.fetch_add(1, Ordering::SeqCst);
        let name = counter.to_string();
        // Generate a UUID v5 (name-based) using the namespace and counter
        Uuid::new_v5(&self.namespace, name.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier};
    use std::thread;

    // Helper function to create a test namespace
    fn create_test_namespace() -> Uuid {
        Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap()
    }

    #[test]
    fn test_uuid_generator_creation() {
        let namespace = create_test_namespace();
        let generator = UuidGenerator::new(namespace);

        assert_eq!(generator.namespace, namespace);
        assert_eq!(generator.counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_uuid_generator_next() {
        let generator = UuidGenerator::new(create_test_namespace());

        // Generate first UUID
        let uuid1 = generator.next();
        assert_eq!(generator.counter.load(Ordering::SeqCst), 1);

        // Generate second UUID
        let uuid2 = generator.next();
        assert_eq!(generator.counter.load(Ordering::SeqCst), 2);

        // UUIDs should be different
        assert_ne!(uuid1, uuid2);

        // Both should be version 5 (name-based) UUIDs
        assert_eq!(uuid1.get_version(), Some(uuid::Version::Sha1));
        assert_eq!(uuid2.get_version(), Some(uuid::Version::Sha1));
    }

    #[test]
    fn test_uuid_generator_deterministic() {
        // Create two generators with the same namespace
        let namespace = create_test_namespace();
        let generator1 = UuidGenerator::new(namespace);
        let generator2 = UuidGenerator::new(namespace);

        // They should generate the same UUIDs for the same counter values
        assert_eq!(generator1.next(), generator2.next());
        assert_eq!(generator1.next(), generator2.next());
    }

    #[test]
    fn test_uuid_generator_different_namespaces() {
        // Create two generators with different namespaces
        let namespace1 = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let namespace2 = Uuid::parse_str("6ba7b811-9dad-11d1-80b4-00c04fd430c8").unwrap();

        let generator1 = UuidGenerator::new(namespace1);
        let generator2 = UuidGenerator::new(namespace2);

        // They should generate different UUIDs for the same counter values
        assert_ne!(generator1.next(), generator2.next());
        assert_ne!(generator1.next(), generator2.next());
    }

    #[test]
    fn test_uuid_generator_sequential() {
        let generator = UuidGenerator::new(create_test_namespace());
        let mut uuids = Vec::new();

        // Generate 100 UUIDs
        for _ in 0..100 {
            uuids.push(generator.next());
        }

        // Check they're all unique
        let unique_uuids: HashSet<_> = uuids.iter().collect();
        assert_eq!(unique_uuids.len(), 100);

        // Check that the counter is properly incremented
        assert_eq!(generator.counter.load(Ordering::SeqCst), 100);
    }

    #[test]
    fn test_uuid_generator_thread_safety() {
        let generator = Arc::new(UuidGenerator::new(create_test_namespace()));
        let num_threads = 10;
        let uuids_per_thread = 100;
        let total_uuids = num_threads * uuids_per_thread;

        // Use a barrier to ensure all threads start at the same time
        let barrier = Arc::new(Barrier::new(num_threads));

        // Shared container to collect all generated UUIDs
        let all_uuids = Arc::new(std::sync::Mutex::new(Vec::with_capacity(total_uuids)));

        let mut handles = vec![];

        for _ in 0..num_threads {
            let thread_generator = Arc::clone(&generator);
            let thread_barrier = Arc::clone(&barrier);
            let thread_uuids = Arc::clone(&all_uuids);

            let handle = thread::spawn(move || {
                thread_barrier.wait(); // Wait for all threads to be ready

                let mut local_uuids = Vec::with_capacity(uuids_per_thread);
                for _ in 0..uuids_per_thread {
                    local_uuids.push(thread_generator.next());
                }

                // Add thread's UUIDs to the shared collection
                let mut all = thread_uuids.lock().unwrap();
                all.extend(local_uuids);
            });

            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Check that all UUIDs are unique
        let all_uuids = all_uuids.lock().unwrap();
        let unique_uuids: HashSet<_> = all_uuids.iter().collect();

        assert_eq!(
            unique_uuids.len(),
            total_uuids,
            "All generated UUIDs should be unique"
        );

        // Verify the counter was incremented correctly
        assert_eq!(
            generator.counter.load(Ordering::SeqCst),
            total_uuids as u64,
            "Counter should match the total number of generated UUIDs"
        );
    }

    #[test]
    fn test_uuid_generator_with_initial_counter() {
        // Create a generator with a custom initial counter value
        let namespace = create_test_namespace();
        let initial_counter = 1000;

        let mut generator = UuidGenerator::new(namespace);
        generator.counter = AtomicU64::new(initial_counter);

        // Generate a UUID
        let _ = generator.next();

        // Verify counter was incremented
        assert_eq!(
            generator.counter.load(Ordering::SeqCst),
            initial_counter + 1
        );

        // Create another generator with initial counter at 1001
        let mut generator2 = UuidGenerator::new(namespace);
        generator2.counter = AtomicU64::new(initial_counter + 1);

        // The next UUID from generator2 should match the next from generator1
        assert_eq!(generator.next(), generator2.next());
    }
}
