//! Library template with documentation setup.

use super::TemplateFile;

pub const FILES: &[TemplateFile] = &[
    TemplateFile {
        path: "{{project_name}}.cabal",
        content: CABAL_FILE,
    },
    TemplateFile {
        path: "hx.toml",
        content: HX_TOML,
    },
    TemplateFile {
        path: ".gitignore",
        content: GITIGNORE,
    },
    TemplateFile {
        path: "README.md",
        content: README,
    },
    TemplateFile {
        path: "src/{{module_name}}.hs",
        content: LIB_HS,
    },
    TemplateFile {
        path: "src/{{module_name}}/Internal.hs",
        content: INTERNAL_HS,
    },
    TemplateFile {
        path: "test/Main.hs",
        content: TEST_MAIN,
    },
    TemplateFile {
        path: "test/{{module_name}}Spec.hs",
        content: TEST_SPEC,
    },
    TemplateFile {
        path: "CHANGELOG.md",
        content: CHANGELOG,
    },
];

const CABAL_FILE: &str = r#"cabal-version: 3.0
name:          {{project_name}}
version:       0.1.0.0
synopsis:      TODO: Write a short synopsis
description:
    TODO: Write a longer description of the library.
    .
    See the README for more information.
license:       MIT
license-file:  LICENSE
author:        {{author}}
maintainer:    {{author}}
copyright:     {{year}} {{author}}
category:      TODO: Choose a category
build-type:    Simple
extra-doc-files:
    CHANGELOG.md
    README.md

source-repository head
    type:     git
    location: https://github.com/username/{{project_name}}

common warnings
    ghc-options: -Wall -Wcompat -Widentities -Wincomplete-record-updates
                 -Wincomplete-uni-patterns -Wmissing-deriving-strategies
                 -Wpartial-fields -Wredundant-constraints
                 -Wmissing-export-lists

library
    import:           warnings
    exposed-modules:
        {{module_name}}
    other-modules:
        {{module_name}}.Internal
    build-depends:
        base ^>=4.18 || ^>=4.19 || ^>=4.20,
        text ^>=2.1,
    hs-source-dirs:   src
    default-language: GHC2021

test-suite {{project_name}}-test
    import:           warnings
    type:             exitcode-stdio-1.0
    main-is:          Main.hs
    other-modules:
        {{module_name}}Spec
    build-depends:
        base ^>=4.18 || ^>=4.19 || ^>=4.20,
        {{project_name}},
        hspec ^>=2.11,
        QuickCheck ^>=2.14,
    hs-source-dirs:   test
    default-language: GHC2021
"#;

const HX_TOML: &str = r#"# hx project configuration
# See: https://github.com/raskell-io/hx

[project]
name = "{{project_name}}"
version = "0.1.0.0"

[toolchain]
# Uncomment to pin GHC version
# ghc = "9.8.2"
{{backend_config}}"#;

const GITIGNORE: &str = r#"# hx build artifacts
.hx/
dist-newstyle/

# Editor files
*.swp
*~
.idea/
.vscode/

# OS files
.DS_Store
"#;

const README: &str = r#"# {{project_name}}

[![Hackage](https://img.shields.io/hackage/v/{{project_name}}.svg)](https://hackage.haskell.org/package/{{project_name}})

TODO: Write a description of your library.

## Installation

Add `{{project_name}}` to your `.cabal` file:

```cabal
build-depends:
    {{project_name}} ^>=0.1
```

## Usage

```haskell
import {{module_name}}

main :: IO ()
main = print $ example "Hello"
```

## Documentation

Full documentation is available on [Hackage](https://hackage.haskell.org/package/{{project_name}}).

## Building

```bash
hx build
```

## Testing

```bash
hx test
```

## Generating Documentation

```bash
cabal haddock
```

## Development

This project was generated with `hx new library`.

## License

MIT License - see LICENSE file.
"#;

const LIB_HS: &str = r#"{-# LANGUAGE OverloadedStrings #-}

-- |
-- Module      : {{module_name}}
-- Copyright   : (c) {{year}} {{author}}
-- License     : MIT
-- Maintainer  : {{author}}
-- Stability   : experimental
-- Portability : GHC
--
-- TODO: Write module documentation here.
--
-- = Quick Start
--
-- @
-- import {{module_name}}
--
-- main :: IO ()
-- main = print $ example "Hello"
-- @
--
module {{module_name}}
    ( -- * Core functions
      example
      -- * Types
    , Result(..)
      -- * Re-exports
    ) where

import {{module_name}}.Internal (Result(..), processInternal)
import Data.Text (Text)

-- | An example function demonstrating the library.
--
-- >>> example "World"
-- Success "Hello, World!"
--
-- @since 0.1.0.0
example :: Text -> Result
example input = processInternal ("Hello, " <> input <> "!")
"#;

const INTERNAL_HS: &str = r#"{-# LANGUAGE DerivingStrategies #-}

-- |
-- Module      : {{module_name}}.Internal
-- Copyright   : (c) {{year}} {{author}}
-- License     : MIT
--
-- Internal implementation details. This module is not part of the
-- public API and may change without notice.
--
module {{module_name}}.Internal
    ( Result(..)
    , processInternal
    ) where

import Data.Text (Text)
import GHC.Generics (Generic)

-- | Result type for operations.
data Result
    = Success Text
    | Failure Text
    deriving stock (Show, Eq, Generic)

-- | Internal processing function.
processInternal :: Text -> Result
processInternal = Success
"#;

const TEST_MAIN: &str = r#"module Main where

import Test.Hspec
import qualified {{module_name}}Spec

main :: IO ()
main = hspec spec

spec :: Spec
spec = do
    {{module_name}}Spec.spec
"#;

const TEST_SPEC: &str = r#"module {{module_name}}Spec (spec) where

import Test.Hspec
import Test.Hspec.QuickCheck
import Test.QuickCheck
import {{module_name}}
import {{module_name}}.Internal (Result(..))

spec :: Spec
spec = do
    describe "example" $ do
        it "returns Success for valid input" $ do
            example "World" `shouldBe` Success "Hello, World!"

        it "handles empty input" $ do
            example "" `shouldBe` Success "Hello, !"

        prop "always returns Success" $ \(input :: String) ->
            case example (fromString input) of
                Success _ -> True
                Failure _ -> False
          where
            fromString = id  -- Simplified for example
"#;

const CHANGELOG: &str = r#"# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0.0] - {{year}}-XX-XX

### Added
- Initial release
- Core `example` function
- Basic documentation
"#;
