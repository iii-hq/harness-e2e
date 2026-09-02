# Fix configuration precedence and cache isolation

Work in `{{workspace}}`. Reproduce the failing public tests before editing. Fix
the precedence rules and cache identity so explicit CLI values override the
environment and separate CLI configurations never share a cached result.

The production correction must involve both `src/config_loader.py` and
`src/config_cache.py`. Do not edit tests. Run the complete public suite and
leave the workspace ready for independent verification.
