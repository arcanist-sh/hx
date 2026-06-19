//! Web application template using Servant.

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
        path: "src/Api.hs",
        content: API_HS,
    },
    TemplateFile {
        path: "src/Server.hs",
        content: SERVER_HS,
    },
    TemplateFile {
        path: "src/Types.hs",
        content: TYPES_HS,
    },
    TemplateFile {
        path: "test/Main.hs",
        content: TEST_MAIN,
    },
];

const CABAL_FILE: &str = r#"cabal-version: 3.0
name:          {{project_name}}
version:       0.1.0.0
synopsis:      A web application built with Servant
license:       MIT
author:        {{author}}
maintainer:    {{author}}
build-type:    Simple

common warnings
    ghc-options: -Wall -Wcompat -Widentities -Wincomplete-record-updates
                 -Wincomplete-uni-patterns -Wmissing-deriving-strategies
                 -Wpartial-fields -Wredundant-constraints
    default-extensions:
        DataKinds
        DeriveAnyClass
        DerivingStrategies
        OverloadedStrings

executable {{project_name}}
    import:           warnings
    main-is:          Main.hs
    build-depends:
        base ^>=4.18 || ^>=4.19 || ^>=4.20,
        {{project_name}},
        warp ^>=3.3,
    hs-source-dirs:   app
    default-language: GHC2021

library
    import:           warnings
    exposed-modules:
        Api
        Server
        Types
    build-depends:
        base ^>=4.18 || ^>=4.19 || ^>=4.20,
        aeson ^>=2.2,
        servant ^>=0.20,
        servant-server ^>=0.20,
        text ^>=2.1,
        wai ^>=3.2,
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
        hspec-wai ^>=0.11,
        servant-server ^>=0.20,
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

A web application built with [Servant](https://docs.servant.dev/).

## Building

```bash
hx build
```

## Running

```bash
hx run
```

The server will start on http://localhost:8080

## API Endpoints

- `GET /health` - Health check endpoint
- `GET /api/hello/:name` - Greeting endpoint

## Testing

```bash
hx test
```

## Development

This project was generated with `hx new webapp`.
"#;

const MAIN_HS: &str = r#"module Main where

import Network.Wai.Handler.Warp (run)
import Server (app)

main :: IO ()
main = do
    putStrLn "Starting server on http://localhost:8080"
    run 8080 app
"#;

const API_HS: &str = r#"-- | API type definition
module Api
    ( API
    , api
    ) where

import Servant
import Types (Greeting)

-- | The full API type
type API =
    "health" :> Get '[JSON] String
    :<|> "api" :> "hello" :> Capture "name" String :> Get '[JSON] Greeting

-- | Proxy for the API type
api :: Proxy API
api = Proxy
"#;

const SERVER_HS: &str = r#"-- | Server implementation
module Server
    ( app
    , server
    ) where

import Servant
import Data.Text (pack)
import Api (API, api)
import Types (Greeting(..))

-- | WAI Application
app :: Application
app = serve api server

-- | Server implementation
server :: Server API
server = healthHandler :<|> helloHandler

-- | Health check handler
healthHandler :: Handler String
healthHandler = pure "OK"

-- | Hello handler
helloHandler :: String -> Handler Greeting
helloHandler name = pure $ Greeting
    { greetingMessage = "Hello, " <> pack name <> "!"
    }
"#;

const TYPES_HS: &str = r#"-- | Shared types
module Types
    ( Greeting(..)
    ) where

import Data.Aeson (ToJSON, FromJSON)
import Data.Text (Text)
import GHC.Generics (Generic)

-- | Greeting response
data Greeting = Greeting
    { greetingMessage :: Text
    }
    deriving stock (Show, Eq, Generic)
    deriving anyclass (ToJSON, FromJSON)
"#;

const TEST_MAIN: &str = r#"module Main where

import Test.Hspec
import Test.Hspec.Wai
import Servant (serve)
import Api (api)
import Server (server)

main :: IO ()
main = hspec spec

spec :: Spec
spec = with (pure $ serve api server) $ do
    describe "GET /health" $ do
        it "responds with 200" $ do
            get "/health" `shouldRespondWith` 200

    describe "GET /api/hello/:name" $ do
        it "responds with greeting" $ do
            get "/api/hello/World" `shouldRespondWith` 200
"#;
