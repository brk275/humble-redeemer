# GLib safety backport

This directory contains the MIT-licensed `glib` 0.18.5 crate from crates.io.
Its original LICENSE and copyright notices are retained. It is a third-party
dependency, separate from Humble Redeemer's independently written application.

Original crate SHA-256:
`233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5`

GTK 3 and WebKitGTK require this GLib binding series. The only source change is
the two-line `VariantStrIter::impl_get` correction from
[upstream PR 1343](https://github.com/gtk-rs/gtk-rs-core/pull/1343): make the
out-pointer mutable and pass `&mut p` to the C function. This addresses
[RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html).

Linux CI exercises the iterator in an optimized regression test. Version-based
audit tools may still identify 0.18.5; review this documented source backport.
Remove the patch when the embedded WebView stack supports a maintained GTK/GLib
binding with the fix included.
