use smallvec::SmallVec;
use std::mem;
use tars_codec::TarsValue;

fn create_smallvec_list(size: usize) -> SmallVec<[Box<TarsValue>; 4]> {
    let mut list = SmallVec::new();
    for i in 0..size {
        list.push(Box::new(TarsValue::Int(i as i32)));
    }
    list
}

#[allow(clippy::vec_box)]
fn create_vec_list(size: usize) -> Vec<Box<TarsValue>> {
    let mut list = Vec::new();
    for i in 0..size {
        list.push(Box::new(TarsValue::Int(i as i32)));
    }
    list
}

fn analyze_allocation_pattern(name: &str, size: usize) {
    println!("\n=== Analyzing {name} with {size} elements ===");

    // Create instances to analyze memory layout
    let smallvec = create_smallvec_list(size);
    let vec = create_vec_list(size);

    // Calculate theoretical allocation counts
    let smallvec_heap_allocs = if size <= 4 { size } else { size + 1 };
    let vec_heap_allocs = size + 1; // Always container + elements

    // Memory usage analysis
    let smallvec_stack_bytes = mem::size_of_val(&smallvec);
    let vec_stack_bytes = mem::size_of_val(&vec);

    println!("Memory Layout Analysis:");
    println!("  SmallVec:");
    if size <= 4 {
        println!("    - Container: {smallvec_stack_bytes} bytes on stack");
        println!("    - Elements:  {size} heap allocations (boxed values)");
        println!("    - Total:     {smallvec_heap_allocs} heap allocations");
    } else {
        println!("    - Container: {smallvec_stack_bytes} bytes on stack + heap allocation");
        println!("    - Elements:  {size} heap allocations (boxed values)");
        println!("    - Total:     {smallvec_heap_allocs} heap allocations");
    }

    println!("  Vec:");
    println!("    - Container: {vec_stack_bytes} bytes on stack (pointer)");
    println!("    - Container: 1 heap allocation (array)");
    println!("    - Elements:  {size} heap allocations (boxed values)");
    println!("    - Total:     {vec_heap_allocs} heap allocations");

    // Calculate efficiency
    let allocation_savings = if vec_heap_allocs > smallvec_heap_allocs {
        (vec_heap_allocs - smallvec_heap_allocs) as f64 / vec_heap_allocs as f64 * 100.0
    } else {
        0.0
    };

    println!("Performance Impact:");
    if allocation_savings > 0.0 {
        println!("  ✅ SmallVec saves {allocation_savings:.1}% heap allocations");
        println!("  ✅ Better cache locality (container on stack)");
        println!("  ✅ Reduced allocator pressure");
    } else {
        println!("  ⚖️  Equivalent heap allocations");
        println!("  ⚠️  SmallVec has higher stack usage");
    }

    // Check if actually using stack storage
    let uses_stack = size <= 4;
    println!(
        "  Stack optimization: {}",
        if uses_stack {
            "✅ Active"
        } else {
            "❌ Fallback to heap"
        }
    );

    // Cleanup to avoid warnings
    drop(smallvec);
    drop(vec);
}

fn simulate_tars_workload() {
    println!("\n=== Simulating Real TARS Workload ===");
    println!("Typical TARS message with multiple small lists");

    // Simulate a typical TARS message structure
    let list_sizes = vec![2, 1, 3, 2, 4, 1]; // Common small list sizes
    let list_sizes_len = list_sizes.len();
    let total_elements: usize = list_sizes.iter().sum();

    println!("Message contains {list_sizes_len} lists with sizes: {list_sizes:?}");
    println!("Total elements across all lists: {total_elements}");

    // Calculate allocation patterns
    let smallvec_allocs: usize = list_sizes
        .iter()
        .map(|&size| if size <= 4 { size } else { size + 1 })
        .sum();

    let vec_allocs: usize = list_sizes
        .iter()
        .map(|&size| size + 1) // Always container + elements
        .sum();

    println!("\nAllocation Analysis:");
    println!("  SmallVec approach: {smallvec_allocs} total heap allocations");
    println!("  Vec approach:      {vec_allocs} total heap allocations");

    let savings = (vec_allocs - smallvec_allocs) as f64 / vec_allocs as f64 * 100.0;
    println!("  SmallVec saves:    {savings:.1}% allocations");
    println!(
        "  Reduction factor:  {:.2}x",
        vec_allocs as f64 / smallvec_allocs as f64
    );

    // Memory pressure analysis
    let container_savings = list_sizes.len(); // Number of container allocations saved
    println!("  Container allocs saved: {container_savings} (stack vs heap)");
}

fn demonstrate_size_threshold() {
    println!("\n=== SmallVec Size Threshold Analysis ===");
    println!("Comparing allocation patterns across different list sizes");

    for size in 1..=8 {
        let smallvec_allocs = if size <= 4 { size } else { size + 1 };
        let vec_allocs = size + 1;
        let savings = ((vec_allocs - smallvec_allocs) as f64 / vec_allocs as f64 * 100.0).max(0.0);

        let status = if size <= 4 {
            format!("✅ {savings:.0}% savings")
        } else {
            "⚖️ Equivalent".to_string()
        };

        println!(
            "  Size {size}: SmallVec={smallvec_allocs} allocs, Vec={vec_allocs} allocs → {status}"
        );
    }

    println!("\nOptimal size choice: 4 elements");
    println!("  - Covers most TARS list use cases (as per comment in types.rs)");
    println!("  - Reasonable stack usage (32 bytes for 4 pointers)");
    println!("  - Significant allocation savings for common cases");
}

fn main() {
    println!("TARS Codec: SmallVec vs Vec Allocation Analysis");
    println!("==============================================");
    println!("Boxing Analysis: Even with Box<TarsValue>, SmallVec provides benefits\n");

    // Test different list sizes
    analyze_allocation_pattern("Single Element", 1);
    analyze_allocation_pattern("Small List", 3);
    analyze_allocation_pattern("Threshold List", 4);
    analyze_allocation_pattern("Medium List", 6);

    simulate_tars_workload();
    demonstrate_size_threshold();
}
