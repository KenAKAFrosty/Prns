#![no_std]
#![forbid(unsafe_code)]
#![doc = "Dependency-free macros shared across the project"]

/// Declares a unit-only enum whose declaration is also the source of its complete `ALL` inventory.
///
/// `ALL` inherits the enum's visibility unless a trailing declaration overrides it or attaches
/// attributes.
///
/// ```
/// prns_macros::iterable_enum! {
///     #[derive(Debug, Clone, Copy, PartialEq, Eq)]
///     enum State {
///         Waiting,
///         Running,
///     }
/// }
///
/// assert_eq!(State::ALL, [State::Waiting, State::Running]);
/// ```
///
/// ```
/// prns_macros::iterable_enum! {
///     pub enum TestState {
///         Waiting,
///         Running,
///     }
///     #[cfg(test)]
///     pub(crate) const ALL;
/// }
/// ```
#[macro_export]
macro_rules! iterable_enum {
    (
        @emit
        $(#[$enum_attribute:meta])*
        $enum_visibility:vis enum $name:ident {
            $(
                $(#[$variant_attribute:meta])*
                $variant:ident $(= $discriminant:expr)?
            ),+ $(,)?
        }
        $(#[$all_attribute:meta])*
        $all_visibility:vis const ALL;
    ) => {
        $(#[$enum_attribute])*
        $enum_visibility enum $name {
            $(
                $(#[$variant_attribute])*
                $variant $(= $discriminant)?,
            )+
        }

        impl $name {
            $(#[$all_attribute])*
            $all_visibility const ALL: [Self; $crate::iterable_enum!(@count $($variant),+)] =
                [$(Self::$variant),+];
        }
    };
    (
        $(#[$enum_attribute:meta])*
        $enum_visibility:vis enum $name:ident {
            $(
                $(#[$variant_attribute:meta])*
                $variant:ident $(= $discriminant:expr)?
            ),+ $(,)?
        }
        $(#[$all_attribute:meta])*
        $all_visibility:vis const ALL;
    ) => {
        $crate::iterable_enum! {
            @emit
            $(#[$enum_attribute])*
            $enum_visibility enum $name {
                $(
                    $(#[$variant_attribute])*
                    $variant $(= $discriminant)?,
                )+
            }
            $(#[$all_attribute])*
            $all_visibility const ALL;
        }
    };
    (
        $(#[$enum_attribute:meta])*
        $enum_visibility:vis enum $name:ident {
            $(
                $(#[$variant_attribute:meta])*
                $variant:ident $(= $discriminant:expr)?
            ),+ $(,)?
        }
    ) => {
        $crate::iterable_enum! {
            @emit
            $(#[$enum_attribute])*
            $enum_visibility enum $name {
                $(
                    $(#[$variant_attribute])*
                    $variant $(= $discriminant)?,
                )+
            }
            $enum_visibility const ALL;
        }
    };
    (@count $($variant:ident),+) => {
        <[()]>::len(&[$($crate::iterable_enum!(@unit $variant)),+])
    };
    (@unit $variant:ident) => { () };
}
