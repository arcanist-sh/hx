//! Numeric/scientific computing template using BHC.

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
        path: "src/Compute.hs",
        content: COMPUTE_HS,
    },
    TemplateFile {
        path: "test/Main.hs",
        content: TEST_MAIN,
    },
];

const CABAL_FILE: &str = r#"cabal-version: 3.0
name:          {{project_name}}
version:       0.1.0.0
synopsis:      A numeric computing project using BHC
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
        Compute
    build-depends:
        base ^>=4.18 || ^>=4.19 || ^>=4.20,
        hmatrix ^>=0.20,
        vector ^>=0.13,
        statistics ^>=0.16,
        massiv ^>=1.0,
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
        QuickCheck ^>=2.14,
    hs-source-dirs:   test
    default-language: GHC2021
"#;

const HX_TOML: &str = r#"# hx project configuration
# See: https://github.com/arcanist-sh/hx

[project]
name = "{{project_name}}"
version = "0.1.0.0"

[toolchain]
# Uncomment to pin GHC version
# ghc = "9.8.2"

[compiler]
backend = "bhc"

[compiler.bhc]
profile = "numeric"
tensor_fusion = true
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

A numeric computing project built with [BHC](https://bhc.dev/) (numeric profile).

## Building

```bash
hx build
```

## Running

```bash
hx run
```

## Testing

```bash
hx test
```

## BHC Numeric Profile

This project is configured to use BHC with the `numeric` profile and tensor
fusion optimizations enabled. This provides:

- Optimized numeric kernel generation
- Tensor fusion for array/matrix operations
- Efficient SIMD vectorization

## Dependencies

- [hmatrix](https://hackage.haskell.org/package/hmatrix) - Linear algebra
- [vector](https://hackage.haskell.org/package/vector) - Efficient arrays
- [statistics](https://hackage.haskell.org/package/statistics) - Statistical functions
- [massiv](https://hackage.haskell.org/package/massiv) - Parallel multi-dimensional arrays

## Development

This project was generated with `hx new numeric`.
"#;

const MAIN_HS: &str = r#"module Main where

import Compute (matrixDemo, statisticsDemo)

main :: IO ()
main = do
    putStrLn "=== Matrix Operations ==="
    matrixDemo

    putStrLn ""
    putStrLn "=== Statistics ==="
    statisticsDemo
"#;

const COMPUTE_HS: &str = r#"-- | Numeric computation module
module Compute
    ( matrixDemo
    , statisticsDemo
    , dotProduct
    , mean
    ) where

import qualified Data.Vector as V

-- | Demonstrate matrix-like operations using vectors.
matrixDemo :: IO ()
matrixDemo = do
    let v1 = V.fromList [1.0, 2.0, 3.0 :: Double]
        v2 = V.fromList [4.0, 5.0, 6.0 :: Double]
        dp = dotProduct v1 v2
    putStrLn $ "  v1 = " ++ show (V.toList v1)
    putStrLn $ "  v2 = " ++ show (V.toList v2)
    putStrLn $ "  dot product = " ++ show dp

-- | Demonstrate basic statistics.
statisticsDemo :: IO ()
statisticsDemo = do
    let xs = V.fromList [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0 :: Double]
        m = mean xs
    putStrLn $ "  data = " ++ show (V.toList xs)
    putStrLn $ "  mean = " ++ show m

-- | Compute the dot product of two vectors.
dotProduct :: V.Vector Double -> V.Vector Double -> Double
dotProduct v1 v2 = V.sum $ V.zipWith (*) v1 v2

-- | Compute the mean of a vector.
mean :: V.Vector Double -> Double
mean xs
    | V.null xs = 0.0
    | otherwise = V.sum xs / fromIntegral (V.length xs)
"#;

const TEST_MAIN: &str = r#"module Main where

import Test.Hspec
import Test.Hspec.QuickCheck
import Test.QuickCheck
import qualified Data.Vector as V
import Compute (dotProduct, mean)

main :: IO ()
main = hspec spec

spec :: Spec
spec = do
    describe "dotProduct" $ do
        it "computes dot product of two vectors" $ do
            let v1 = V.fromList [1.0, 2.0, 3.0]
                v2 = V.fromList [4.0, 5.0, 6.0]
            dotProduct v1 v2 `shouldBe` 32.0

        it "returns 0 for empty vectors" $ do
            dotProduct V.empty V.empty `shouldBe` 0.0

    describe "mean" $ do
        it "computes mean of a vector" $ do
            let xs = V.fromList [2.0, 4.0, 6.0]
            mean xs `shouldBe` 4.0

        it "returns 0 for empty vector" $ do
            mean V.empty `shouldBe` 0.0

        prop "mean of singleton is the element itself" $ \(x :: Double) ->
            mean (V.singleton x) == x
"#;
