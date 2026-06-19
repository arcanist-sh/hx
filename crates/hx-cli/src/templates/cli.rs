//! CLI application template using optparse-applicative.

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
        path: "app/Main.hs",
        content: MAIN_HS,
    },
    TemplateFile {
        path: "src/Cli.hs",
        content: CLI_HS,
    },
    TemplateFile {
        path: "src/App.hs",
        content: APP_HS,
    },
    TemplateFile {
        path: "test/Main.hs",
        content: TEST_MAIN,
    },
];

const CABAL_FILE: &str = r#"cabal-version: 3.0
name:          {{project_name}}
version:       0.1.0.0
synopsis:      A command-line application
license:       MIT
author:        {{author}}
maintainer:    {{author}}
build-type:    Simple

common warnings
    ghc-options: -Wall -Wcompat -Widentities -Wincomplete-record-updates
                 -Wincomplete-uni-patterns -Wmissing-deriving-strategies
                 -Wpartial-fields -Wredundant-constraints

executable {{project_name}}
    import:           warnings
    main-is:          Main.hs
    build-depends:
        base ^>=4.18 || ^>=4.19 || ^>=4.20,
        {{project_name}},
    hs-source-dirs:   app
    default-language: GHC2021

library
    import:           warnings
    exposed-modules:
        Cli
        App
    build-depends:
        base ^>=4.18 || ^>=4.19 || ^>=4.20,
        optparse-applicative ^>=0.18,
        text ^>=2.1,
    hs-source-dirs:   src
    default-language: GHC2021

test-suite {{project_name}}-test
    import:           warnings
    type:             exitcode-stdio-1.0
    main-is:          Main.hs
    build-depends:
        base ^>=4.18 || ^>=4.19 || ^>=4.20,
        {{project_name}},
        hspec ^>=2.11,
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

A command-line application built with [optparse-applicative](https://hackage.haskell.org/package/optparse-applicative).

## Building

```bash
hx build
```

## Running

```bash
hx run -- --help
hx run -- greet --name "World"
hx run -- version
```

## Commands

- `greet` - Print a greeting message
- `version` - Show version information

## Testing

```bash
hx test
```

## Development

This project was generated with `hx new cli`.
"#;

const MAIN_HS: &str = r#"module Main where

import Cli (run)

main :: IO ()
main = run
"#;

const CLI_HS: &str = r#"{-# LANGUAGE DerivingStrategies #-}

-- | Command-line interface definition
module Cli
    ( run
    , Options(..)
    , Command(..)
    ) where

import Options.Applicative
import App (greet, showVersion)

-- | Main entry point
run :: IO ()
run = do
    opts <- execParser optsParser
    case optCommand opts of
        Greet name -> greet name
        Version -> showVersion

-- | Top-level options
data Options = Options
    { optVerbose :: Bool
    , optCommand :: Command
    }
    deriving stock (Show, Eq)

-- | Available commands
data Command
    = Greet String
    | Version
    deriving stock (Show, Eq)

-- | Parser for options
optsParser :: ParserInfo Options
optsParser = info (helper <*> versionOption <*> programOptions)
    ( fullDesc
    <> header "{{project_name}} - A command-line application"
    <> progDesc "Example CLI application with subcommands"
    )

versionOption :: Parser (a -> a)
versionOption = infoOption "{{project_name}} 0.1.0.0"
    ( long "version"
    <> short 'V'
    <> help "Show version information"
    )

programOptions :: Parser Options
programOptions = Options
    <$> switch
        ( long "verbose"
        <> short 'v'
        <> help "Enable verbose output"
        )
    <*> commandParser

commandParser :: Parser Command
commandParser = subparser
    ( command "greet" (info greetCommand (progDesc "Print a greeting"))
    <> command "version" (info (pure Version) (progDesc "Show version"))
    )

greetCommand :: Parser Command
greetCommand = Greet
    <$> strOption
        ( long "name"
        <> short 'n'
        <> metavar "NAME"
        <> help "Name to greet"
        <> value "World"
        <> showDefault
        )
"#;

const APP_HS: &str = r#"{-# LANGUAGE OverloadedStrings #-}

-- | Application logic
module App
    ( greet
    , showVersion
    ) where

import Data.Text (pack)
import qualified Data.Text.IO as T

-- | Print a greeting
greet :: String -> IO ()
greet name = T.putStrLn $ "Hello, " <> pack name <> "!"

-- | Show version information
showVersion :: IO ()
showVersion = putStrLn "{{project_name}} version 0.1.0.0"
"#;

const TEST_MAIN: &str = r#"module Main where

import Test.Hspec
import Cli (Command(..), Options(..))

main :: IO ()
main = hspec spec

spec :: Spec
spec = do
    describe "Commands" $ do
        it "Greet command stores name" $ do
            let cmd = Greet "Test"
            cmd `shouldBe` Greet "Test"

        it "Version command exists" $ do
            Version `shouldBe` Version
"#;
