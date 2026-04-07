# Agentic Changelog

- Improved `morbid-app` database architecture by establishing the `chunks` table during startup initialization, importing `ReadableDatabase` for `redb 4.0.0` read transactions, and simplifying chunk loads to rely on the startup schema invariant instead of handling table bootstrap in the read path.
