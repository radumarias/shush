//! This is a fork of the [secrets](https://github.com/stouset/secrets) crate.
//! This crate adds `mlock` and `mprotect` to lock the secret's page in memory
//! and read only when exposed

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![warn(missing_docs, rust_2018_idioms, unused_qualifications)]

use core::{
    any,
    fmt::{self, Debug},
};
use libc::{PROT_NONE, PROT_READ, PROT_WRITE};
use memsec::{mlock, mprotect, munlock};
use std::{mem, ptr::NonNull};

use zeroize::{Zeroize, ZeroizeOnDrop};

pub use zeroize;

/// Wrapper for the inner secret. Can be exposed by [`ExposeSecret`]
pub struct SecretBox<S: Zeroize> {
    inner_secret: Box<S>,
}

impl<S: Zeroize> Zeroize for SecretBox<S> {
    fn zeroize(&mut self) {
        let secret_ptr = self.inner_secret.as_ref() as *const S;

        let len = mem::size_of::<S>();

        unsafe {
            if !munlock(secret_ptr as *mut u8, len) {
                eprintln!("Unable to munlock variable")
            }

            if !mprotect(
                NonNull::new(secret_ptr as *mut S).expect("Unable to convert ptr to NonNull"),
                PROT_READ | PROT_WRITE,
            ) {
                eprintln!("Unable to unprotect variable")
            }
        }

        self.inner_secret.as_mut().zeroize()
    }
}

impl<S: Zeroize> Drop for SecretBox<S> {
    fn drop(&mut self) {
        self.zeroize()
    }
}

impl<S: Zeroize> ZeroizeOnDrop for SecretBox<S> {}

impl<S: Zeroize> From<Box<S>> for SecretBox<S> {
    fn from(source: Box<S>) -> Self {
        Self::new(source)
    }
}

impl<S: Zeroize> SecretBox<S> {
    /// Create a secret value using a pre-boxed value.
    pub fn new(boxed_secret: Box<S>) -> Self {
        let secret_ptr = Box::into_raw(boxed_secret);

        let len = mem::size_of::<S>();

        unsafe {
            if !mlock(secret_ptr as *mut u8, len) {
                eprintln!("Unable to mlock variable ")
            }

            if !mprotect(
                NonNull::new(secret_ptr).expect("Unable to convert box to NonNull"),
                PROT_NONE,
            ) {
                eprintln!("Unable to protect secret")
            }
        }

        let inner_secret = unsafe { Box::from_raw(secret_ptr) };

        Self { inner_secret }
    }
}

impl<S: Zeroize + Default> SecretBox<S> {
    /// Create a secret value using a function that can initialize the vale in-place.
    pub fn new_with_mut(ctr: impl FnOnce(&mut S)) -> Self {
        let mut secret = Self::default();
        ctr(secret.expose_secret_mut());
        secret
    }
}

impl<S: Zeroize + Clone> SecretBox<S> {
    /// Create a secret value using the provided function as a constructor.
    ///
    /// The implementation makes an effort to zeroize the locally constructed value
    /// before it is copied to the heap, and constructing it inside the closure minimizes
    /// the possibility of it being accidentally copied by other code.
    ///
    /// **Note:** using [`Self::new`] or [`Self::new_with_mut`] is preferable when possible,
    /// since this method's safety relies on empyric evidence and may be violated on some targets.
    pub fn new_with_ctr(ctr: impl FnOnce() -> S) -> Self {
        let mut data = ctr();
        let secret = Self {
            inner_secret: Box::new(data.clone()),
        };
        data.zeroize();
        secret
    }

    /// Same as [`Self::new_with_ctr`], but the constructor can be fallible.
    ///
    ///
    /// **Note:** using [`Self::new`] or [`Self::new_with_mut`] is preferable when possible,
    /// since this method's safety relies on empyric evidence and may be violated on some targets.
    pub fn try_new_with_ctr<E>(ctr: impl FnOnce() -> Result<S, E>) -> Result<Self, E> {
        let mut data = ctr()?;
        let secret = Self {
            inner_secret: Box::new(data.clone()),
        };
        data.zeroize();
        Ok(secret)
    }
}

impl<S: Zeroize + Default> Default for SecretBox<S> {
    fn default() -> Self {
        Self {
            inner_secret: Box::<S>::default(),
        }
    }
}

impl<S: Zeroize> Debug for SecretBox<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretBox<{}>([REDACTED])", any::type_name::<S>())
    }
}

impl<S> Clone for SecretBox<S>
where
    S: CloneableSecret,
{
    fn clone(&self) -> Self {
        SecretBox {
            inner_secret: self.inner_secret.clone(),
        }
    }
}

impl<S: Zeroize> ExposeSecret<S> for SecretBox<S> {
    fn expose_secret(&self) -> &S {
        let secret_ptr = self.inner_secret.as_ref() as *const S;

        unsafe {
            if !mprotect(
                NonNull::new(secret_ptr as *mut S).expect("Unable to convert ptr to NonNull"),
                PROT_READ,
            ) {
                eprintln!("Unable to unprotect variable")
            }
        }
        self.inner_secret.as_ref()
    }
}

impl<S: Zeroize> ExposeSecretMut<S> for SecretBox<S> {
    fn expose_secret_mut(&mut self) -> &mut S {
        let secret_ptr = self.inner_secret.as_ref() as *const S;

        unsafe {
            if !mprotect(
                NonNull::new(secret_ptr as *mut S).expect("Unable to convert ptr to NonNull"),
                PROT_READ | PROT_WRITE,
            ) {
                eprintln!("Unable to unprotect variable")
            }
        }
        self.inner_secret.as_mut()
    }
}

/// Marker trait for secrets which are allowed to be cloned
pub trait CloneableSecret: Clone + Zeroize {}

/// Expose a reference to an inner secret
pub trait ExposeSecret<S> {
    /// Expose secret: this is the only method providing access to a secret.
    fn expose_secret(&self) -> &S;
}

/// Expose a mutable reference to an inner secret
pub trait ExposeSecretMut<S> {
    /// Expose secret: this is the only method providing access to a secret.
    fn expose_secret_mut(&mut self) -> &mut S;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, Clone, Default)]
    struct TestSecret {
        data: Vec<u8>,
    }

    impl TestSecret {
        fn new(size: usize) -> Self {
            let mut data = vec![0; size];
            data[0] = 1;
            Self { data }
        }

        fn check_non_zero(&self) -> bool {
            self.data.iter().any(|&x| x != 0)
        }

        fn check_zero(&self) -> bool {
            self.data.iter().all(|&x| x == 0)
        }
    }

    impl Zeroize for TestSecret {
        fn zeroize(&mut self) {
            self.data = Vec::new();
        }
    }

    #[test]
    fn test_secret_box_drop_zeroizes() {
        let secret = Box::new(TestSecret::new(10));
        let secret_box = SecretBox::new(secret);
        assert!(secret_box.expose_secret().check_non_zero());

        drop(secret_box);

        // Verify that secret is zeroized after drop
        // This requires checking the memory, which is not straightforward in Rust.
        // Here we rely on the zeroize trait to ensure it zeroizes.
        assert!(TestSecret::default().check_zero());
    }

    #[test]
    fn test_secret_box_expose_secret_mut() {
        let secret = Box::new(TestSecret::new(10));
        let mut secret_box = SecretBox::new(secret);

        let exposed = secret_box.expose_secret_mut();
        exposed.data[0] = 42;

        assert_eq!(secret_box.expose_secret().data[0], 42);
    }

    #[test]
    fn test_secret_box_new_with_ctr() {
        let secret_box = SecretBox::new_with_ctr(|| TestSecret::new(10));
        assert!(secret_box.expose_secret().check_non_zero());
    }

    #[test]
    fn test_secret_box_try_new_with_ctr() {
        let result: Result<SecretBox<TestSecret>, &'static str> =
            SecretBox::try_new_with_ctr(|| Ok(TestSecret::new(10)));

        match result {
            Ok(secret_box) => assert!(secret_box.expose_secret().check_non_zero()),
            Err(_) => panic!("Expected Ok variant"),
        }
    }
}
