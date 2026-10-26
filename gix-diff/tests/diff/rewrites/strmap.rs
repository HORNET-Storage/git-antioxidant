use gix_diff::rewrites::strmap::iteration_order;

// The expected orders are those that git 2.55.0 visits: a program linked with its `libgit.a` read the keys one per line,
// added each to an empty `strintmap` with `strintmap_set()`, its line number as the value, and printed the values in the
// order in which `strintmap_for_each_entry()` visited them.

#[test]
fn keys_are_visited_by_bucket_and_the_last_one_added_first_within_a_bucket() {
    let keys = [
        "src",
        "",
        "a",
        "docs/api",
        "!",
        "/",
        "tests/fixtures",
        "lib",
        "x",
        "y/z",
        "crates/gix-merge/src",
        "very/long/directory/name/with/many/components",
    ];
    assert_eq!(
        iteration_order(keys),
        [6, 7, 1, 11, 0, 8, 10, 5, 9, 3, 4, 2],
        "`!` and `a` share a bucket, in which `!`, added later, comes first"
    );
}

#[test]
fn keys_are_moved_to_more_buckets_as_git_moves_them() {
    let keys: Vec<_> = (0..60).map(|key| format!("dir-{key}")).collect();
    assert_eq!(
        iteration_order(&keys),
        [
            21, 20, 23, 22, 25, 24, 27, 26, 29, 28, 18, 19, 10, 11, 12, 13, 14, 15, 16, 17, 47, 46, 45, 44, 43, 42, 41,
            40, 49, 48, 54, 55, 56, 57, 50, 51, 52, 53, 58, 59, 38, 39, 36, 37, 34, 35, 32, 33, 30, 31, 9, 8, 1, 0, 3,
            2, 5, 4, 7, 6
        ],
        "the map grows from 64 to 256 buckets as the 52nd key is added"
    );

    let keys: Vec<_> = (0..210).map(|key| format!("d/{key}")).collect();
    assert_eq!(
        iteration_order(&keys),
        [
            39, 38, 33, 32, 31, 30, 37, 36, 35, 34, 48, 49, 42, 43, 40, 41, 46, 47, 44, 45, 172, 173, 170, 171, 176,
            177, 174, 175, 178, 179, 103, 102, 101, 100, 107, 106, 105, 104, 109, 108, 64, 65, 66, 67, 60, 61, 62, 63,
            68, 69, 86, 87, 84, 85, 82, 158, 83, 159, 80, 81, 154, 155, 156, 157, 150, 151, 88, 152, 89, 153, 189, 188,
            183, 182, 181, 180, 187, 186, 185, 184, 208, 209, 200, 201, 202, 203, 204, 205, 206, 207, 59, 58, 55, 54,
            57, 56, 51, 50, 53, 52, 165, 164, 167, 166, 161, 160, 163, 162, 169, 168, 136, 137, 134, 135, 132, 133,
            130, 131, 138, 139, 28, 29, 20, 21, 22, 23, 24, 25, 26, 27, 77, 76, 75, 74, 73, 72, 71, 70, 79, 78, 110,
            111, 112, 113, 114, 115, 116, 117, 118, 119, 99, 98, 149, 148, 91, 147, 90, 146, 93, 145, 92, 144, 95, 143,
            94, 142, 97, 141, 96, 140, 11, 10, 13, 12, 15, 14, 17, 16, 19, 18, 129, 128, 121, 120, 123, 122, 125, 124,
            127, 126, 198, 199, 8, 9, 2, 190, 3, 191, 0, 192, 1, 193, 6, 194, 7, 195, 4, 196, 5, 197
        ],
        "the map grows to 256 buckets as the 52nd key is added and to 1024 as the 205th is"
    );
}
