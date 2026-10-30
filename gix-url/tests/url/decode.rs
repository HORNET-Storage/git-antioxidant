use bstr::ByteSlice;

#[test]
fn escapes_are_decoded_after_the_first_colon_as_in_gits_url_decode_mem() {
    for (input, expected) in [
        ("https://h%6fst/p%41th", "https://host/pAth"),
        ("a%6Fb://c", "a%6Fb://c"),
        ("no-colon%2fat%2Fall", "no-colon/at/all"),
        (":%41", ":A"),
        ("x:%0a", "x:\n"),
        ("x:%00", "x:%00"),
        ("x:%4", "x:%4"),
        ("x:%", "x:%"),
        ("x:%zz%41", "x:%zzA"),
        ("x:%%41", "x:%A"),
        ("", ""),
    ] {
        assert_eq!(gix_url::decode(input.as_bytes()), expected, "{input:?}");
    }
    assert_eq!(
        gix_url::decode(b"x:%ff\xfe").as_bstr(),
        b"x:\xff\xfe".as_bstr(),
        "bytes don't have to be text"
    );
}
