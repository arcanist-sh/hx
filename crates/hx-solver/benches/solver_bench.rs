//! Benchmarks for the dependency solver.
//!
//! Run with: cargo bench -p hx-solver

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hx_solver::{
    Dependency, PackageIndex, PackageVersion, Resolver, ResolverConfig, VersionConstraint,
};
use std::hint::black_box;

/// Create a synthetic package index with N packages, each with M versions.
fn create_synthetic_index(num_packages: usize, versions_per_package: usize) -> PackageIndex {
    let mut index = PackageIndex::new();

    // Create base package
    let base_version = PackageVersion::new("base", "4.18.0".parse().unwrap());
    index.add_package_version(base_version);

    // Create synthetic packages with dependencies
    for i in 0..num_packages {
        let pkg_name = format!("pkg-{}", i);

        for v in 0..versions_per_package {
            let version_str = format!("{}.0.0", v + 1);
            let mut pv = PackageVersion::new(&pkg_name, version_str.parse().unwrap());

            // Add dependency on base
            pv.add_dependency(Dependency::with_constraint(
                "base",
                VersionConstraint::GreaterThanOrEqual("4.0".parse().unwrap()),
            ));

            // Add dependency on previous package (if not first)
            if i > 0 {
                let dep_name = format!("pkg-{}", i - 1);
                pv.add_dependency(Dependency::with_constraint(
                    &dep_name,
                    VersionConstraint::Any,
                ));
            }

            index.add_package_version(pv);
        }
    }

    index
}

/// Benchmark simple resolution (few packages, few versions).
fn bench_resolve_simple(c: &mut Criterion) {
    let index = create_synthetic_index(10, 3);
    let resolver = Resolver::new(&index);

    c.bench_function("resolve_simple_10pkg_3ver", |b| {
        b.iter(|| {
            resolver
                .resolve(black_box("pkg-9"), black_box(&VersionConstraint::Any))
                .unwrap()
        })
    });
}

/// Benchmark resolution with different package counts.
fn bench_resolve_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolve_scaling");

    for num_packages in [5, 10, 20, 50].iter() {
        let index = create_synthetic_index(*num_packages, 5);
        let resolver = Resolver::new(&index);
        let target = format!("pkg-{}", num_packages - 1);

        group.bench_with_input(
            BenchmarkId::new("packages", num_packages),
            num_packages,
            |b, _| {
                b.iter(|| {
                    resolver
                        .resolve(black_box(&target), black_box(&VersionConstraint::Any))
                        .unwrap()
                })
            },
        );
    }

    group.finish();
}

/// Benchmark resolution with different version counts.
fn bench_resolve_version_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolve_version_scaling");

    for versions in [3, 5, 10, 20].iter() {
        let index = create_synthetic_index(20, *versions);
        let resolver = Resolver::new(&index);

        group.bench_with_input(
            BenchmarkId::new("versions_per_pkg", versions),
            versions,
            |b, _| {
                b.iter(|| {
                    resolver
                        .resolve(black_box("pkg-19"), black_box(&VersionConstraint::Any))
                        .unwrap()
                })
            },
        );
    }

    group.finish();
}

/// Benchmark multiple dependency resolution.
fn bench_resolve_all(c: &mut Criterion) {
    let index = create_synthetic_index(30, 5);
    let resolver = Resolver::new(&index);

    let deps: Vec<Dependency> = (0..10)
        .map(|i| Dependency::with_constraint(format!("pkg-{}", i * 3), VersionConstraint::Any))
        .collect();

    c.bench_function("resolve_all_10_deps", |b| {
        b.iter(|| resolver.resolve_all(black_box(&deps)).unwrap())
    });
}

/// Benchmark resolver with prefer_newest = false.
fn bench_resolve_prefer_oldest(c: &mut Criterion) {
    let index = create_synthetic_index(20, 10);

    let config = ResolverConfig {
        prefer_newest: false,
        ..Default::default()
    };
    let resolver = Resolver::with_config(&index, config);

    c.bench_function("resolve_prefer_oldest_20pkg", |b| {
        b.iter(|| {
            resolver
                .resolve(black_box("pkg-19"), black_box(&VersionConstraint::Any))
                .unwrap()
        })
    });
}

/// Benchmark package index creation.
fn bench_index_creation(c: &mut Criterion) {
    c.bench_function("index_create_100pkg_10ver", |b| {
        b.iter(|| black_box(create_synthetic_index(100, 10)))
    });
}

criterion_group!(
    benches,
    bench_resolve_simple,
    bench_resolve_scaling,
    bench_resolve_version_scaling,
    bench_resolve_all,
    bench_resolve_prefer_oldest,
    bench_index_creation,
);

criterion_main!(benches);
