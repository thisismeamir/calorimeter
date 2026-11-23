use calo_core::symbols::{
    demangle_symbol, fold_templates, FakeSymbolizer, Normalizer, RawFrame, SymbolError, Symbolizer,
};
use calo_schema::suite::{SymbolClassRule, SymbolFoldRule};

#[test]
fn demangle_itanium_and_rust_symbols_with_fallback() {
    // Real Itanium C++ symbol
    let mangled_cpp = "_ZNKSt6vectorIdSaIdEE4sizeEv";
    let dem = demangle_symbol(mangled_cpp);
    assert!(dem.contains("vector"), "got: {dem}");

    // Template-heavy Itanium symbol
    let heavy = "_ZN6Kokkos4Impl14ParallelReduceINS0_11RangePolicyIiEEdE7executeEv";
    let dem_heavy = demangle_symbol(heavy);
    println!("{:?}", demangle_symbol(heavy));
    assert!(
        dem_heavy.contains("Kokkos::Impl::ParallelReduce"),
        "got: {dem_heavy}"
    );
    assert!(dem_heavy.contains("Kokkos::Impl::ParallelReduce"));

    // Rust legacy symbol
    let rust_sym =
        "_ZN4core3ptr46drop_in_place$LT$alloc..vec..Vec$LT$u8$GT$$GT$17h1234567890abcdefE";
    let dem_rust = demangle_symbol(rust_sym);
    assert!(dem_rust.contains("core::ptr::drop_in_place"));

    // Unmangled / invalid fallback
    assert_eq!(demangle_symbol("plain_c_func"), "plain_c_func");
    assert_eq!(demangle_symbol("_Zinvalid_mangled"), "_Zinvalid_mangled");
}

#[test]
fn fold_templates_nested_and_unbalanced() {
    let nested =
        "Kokkos::Impl::ParallelFor<Kokkos::RangePolicy<Kokkos::Serial>, MyFunctor<double, 3>>";
    assert_eq!(
        fold_templates(nested, 1),
        "Kokkos::Impl::ParallelFor<Kokkos::RangePolicy<...>, MyFunctor<...>>"
    );
    assert_eq!(fold_templates(nested, 0), "Kokkos::Impl::ParallelFor<...>");

    // Operator tokens and unbalanced brackets never panic or corrupt
    assert_eq!(
        fold_templates("MyType::operator<<(std::ostream&, int)", 1),
        "MyType::operator<<(std::ostream&, int)"
    );
    assert_eq!(
        fold_templates("unbalanced<template<T>", 1),
        "unbalanced<template<T>"
    );
    assert_eq!(fold_templates("stray>bracket", 1), "stray>bracket");
}

#[test]
fn normalizer_fold_order_first_match_classes_and_builtins() {
    let fold = vec![
        SymbolFoldRule {
            match_regex: r"Kokkos::Impl::[^<]*<.*>".to_string(),
            replace_as: "Kokkos::<impl>".to_string(),
        },
        SymbolFoldRule {
            match_regex: r"\(anonymous namespace\)::".to_string(),
            replace_as: "".to_string(),
        },
    ];
    let classes = vec![
        SymbolClassRule {
            match_regex: r"cufft|fftw".to_string(),
            class: "fft".to_string(),
        },
        SymbolClassRule {
            match_regex: r"PMPI_".to_string(),
            class: "custom_pmpi".to_string(),
        },
    ];

    let norm = Normalizer::new(&fold, &classes).unwrap();

    // First-match user class wins over built-in `mpi`
    let f_pmpi = norm.normalize(RawFrame {
        symbol: "PMPI_Allreduce".to_string(),
        file: None,
        line: None,
    });
    assert_eq!(f_pmpi.class.as_deref(), Some("custom_pmpi"));

    // Built-in classes
    assert_eq!(norm.classify_name("MPI_Send"), Some("mpi"));
    assert_eq!(norm.classify_name("GOMP_parallel"), Some("omp"));
    assert_eq!(norm.classify_name("__kmpc_fork_call"), Some("omp"));
    assert_eq!(norm.classify_name("malloc"), Some("memory"));
    assert_eq!(
        norm.classify_name("operator new(unsigned long)"),
        Some("memory")
    );
    assert_eq!(norm.classify_name("H5Dwrite"), Some("io"));
    assert_eq!(norm.classify_name("cudaMemcpyAsync"), Some("memcpy"));
    assert_eq!(norm.classify_name("hipDeviceSynchronize"), Some("sync"));
    assert_eq!(norm.classify_gpu_kernel("step_evolve_kernel"), "compute");

    // FakeSymbolizer integration
    let mut sym = FakeSymbolizer::new();
    sym.insert(
        "abcd",
        0x401000,
        RawFrame {
            symbol: "(anonymous namespace)::cufftExecZ2Z".to_string(),
            file: Some("fft.cu".to_string()),
            line: Some(42),
        },
    );
    let raw = sym.resolve("abcd", 0x401000).unwrap();
    let frame = norm.normalize(raw);
    assert_eq!(frame.name, "cufftExecZ2Z");
    assert_eq!(frame.class.as_deref(), Some("fft"));
}

#[test]
fn invalid_regex_returns_indexed_error() {
    let bad_fold = vec![SymbolFoldRule {
        match_regex: "[unclosed".to_string(),
        replace_as: "x".to_string(),
    }];
    let err = Normalizer::new(&bad_fold, &[])
        .err()
        .expect("invalid regex must fail");
    assert!(matches!(
        err,
        SymbolError::InvalidRegex {
            kind: "fold",
            index: 0,
            ..
        }
    ));
}

#[test]
#[ignore]
fn performance_sanity_100k_names() {
    let norm = Normalizer::new(&[], &[]).unwrap().with_fold_templates(2);
    let start = std::time::Instant::now();
    for i in 0..100_000 {
        let _ = norm.normalize(RawFrame {
            symbol: format!("MPI_Allreduce_{i}"),
            file: None,
            line: None,
        });
    }
    assert!(start.elapsed().as_secs_f64() < 5.0);
}
