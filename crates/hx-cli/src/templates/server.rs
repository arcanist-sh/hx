//! Server application template using BHC with Servant.

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
        path: "src/Config.hs",
        content: CONFIG_HS,
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
synopsis:      A server application built with Servant and BHC
license:       MIT
author:        {{author}}
maintainer:    {{author}}
build-type:    Simple

common warnings
    ghc-options: -Wall -Wcompat -Widentities -Wincomplete-record-updates
                 -Wincomplete-uni-patterns -Wmissing-deriving-strategies
                 -Wpartial-fields -Wredundant-constraints
    default-extensions:
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
    ghc-options:      -threaded -rtsopts -with-rtsopts=-N

library
    import:           warnings
    exposed-modules:
        Api
        Server
        Config
        Types
    build-depends:
        base ^>=4.18 || ^>=4.19 || ^>=4.20,
        aeson ^>=2.2,
        servant ^>=0.20,
        servant-server ^>=0.20,
        text ^>=2.1,
        wai ^>=3.2,
        wai-extra ^>=3.1,
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

[compiler]
backend = "bhc"

[compiler.bhc]
profile = "server"
"#;

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

A server application built with [Servant](https://docs.servant.dev/) and
[BHC](https://bhc.dev/) (server profile).

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

## Configuration

The server reads configuration from environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `8080` | Server port |
| `HOST` | `0.0.0.0` | Server host |

## BHC Server Profile

This project uses BHC with the `server` profile, which provides:

- Optimized runtime for long-running server processes
- Improved GC behavior for request/response workloads
- Efficient concurrent I/O handling

## Development

This project was generated with `hx new server`.
"#;

const MAIN_HS: &str = r#"module Main where

import Network.Wai.Handler.Warp (run)
import Config (loadConfig, configPort)
import Server (app)

main :: IO ()
main = do
    config <- loadConfig
    let port = configPort config
    putStrLn $ "Starting server on http://localhost:" ++ show port
    run port app
"#;

const API_HS: &str = r#"-- | API type definition
module Api
    ( API
    , api
    ) where

import Servant
import Types (Greeting, HealthStatus)

-- | The full API type
type API =
    "health" :> Get '[JSON] HealthStatus
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
import Network.Wai (Middleware)
import Network.Wai.Middleware.RequestLogger (logStdoutDev)
import Api (API, api)
import Types (Greeting(..), HealthStatus(..))

-- | WAI Application with logging middleware
app :: Application
app = logStdoutDev $ serve api server

-- | Server implementation
server :: Server API
server = healthHandler :<|> helloHandler

-- | Health check handler
healthHandler :: Handler HealthStatus
healthHandler = pure $ HealthStatus
    { hsStatus = "ok"
    , hsVersion = "0.1.0.0"
    }

-- | Hello handler
helloHandler :: String -> Handler Greeting
helloHandler name = pure $ Greeting
    { greetingMessage = "Hello, " <> name <> "!"
    }
"#;

const CONFIG_HS: &str = r#"-- | Environment-based configuration
module Config
    ( AppConfig(..)
    , loadConfig
    , configPort
    ) where

import System.Environment (lookupEnv)
import System.IO (hPutStrLn, stderr)

-- | Application configuration
data AppConfig = AppConfig
    { appPort :: Int
    , appHost :: String
    }
    deriving stock (Show, Eq)

-- | Default configuration
defaultConfig :: AppConfig
defaultConfig = AppConfig
    { appPort = 8080
    , appHost = "0.0.0.0"
    }

-- | Load configuration from environment variables
loadConfig :: IO AppConfig
loadConfig = do
    port <- lookupEnvDefault "PORT" (appPort defaultConfig) readMaybe
    host <- lookupEnvDefault "HOST" (appHost defaultConfig) Just
    pure AppConfig { appPort = port, appHost = host }

-- | Get the configured port
configPort :: AppConfig -> Int
configPort = appPort

-- | Look up an environment variable with a default value
lookupEnvDefault :: String -> a -> (String -> Maybe a) -> IO a
lookupEnvDefault key def parser = do
    result <- lookupEnv key
    case result of
        Nothing -> pure def
        Just val -> case parser val of
            Just parsed -> pure parsed
            Nothing -> do
                hPutStrLn stderr $ "Warning: invalid value for " ++ key ++ ": " ++ val
                pure def

-- | Simple read parser
readMaybe :: Read a => String -> Maybe a
readMaybe s = case reads s of
    [(x, "")] -> Just x
    _ -> Nothing
"#;

const TYPES_HS: &str = r#"-- | Shared types
module Types
    ( Greeting(..)
    , HealthStatus(..)
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

-- | Health check response
data HealthStatus = HealthStatus
    { hsStatus :: Text
    , hsVersion :: Text
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
