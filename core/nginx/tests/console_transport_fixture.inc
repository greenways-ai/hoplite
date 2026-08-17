#define HTA_NIL 0u
#define HTA_FALSE 1u
#define HTA_TRUE 2u
#define HTA_I64 3u
#define HTA_STRING 4u
#define HTA_BYTES 5u
#define HTA_KEYWORD 6u
#define HTA_SYMBOL 7u
#define HTA_LIST 8u
#define HTA_VECTOR 9u
#define HTA_SET 10u
#define HTA_MAP 11u
#define HTA_HANDLE 12u
#define HTA_NAMESPACE 13u
#define HTA_VAR 14u
#define HTA_F64 15u
#define HTA_ATOM 16u
#define HTA_ARRAY 17u
#define HTA_OBJECT 18u
#define HTA_CHARACTER 19u
#define HTA_BIG_INTEGER 20u
#define HTA_DECIMAL 21u
#define HTA_REGEX 22u
#define HTA_TUPLE 23u
#define HTA_CONS 24u
#define HTA_QUEUE 25u
#define HTA_ORDERED_MAP 26u
#define HTA_SORTED_MAP 27u
#define HTA_TRIE 28u
#define HTA_ORDERED_SET 29u
#define HTA_SORTED_SET 30u
#define HTA_TAGGED 31u
#define HTA_EXCEPTION_INFO 32u
#define HTA_STRUCT 33u
#define HTA_POINTER 34u
#define HTA_VAR_REF 35u
#define HTA_DEQUE 36u
#define HTA_PRIORITY_MAP 37u

static int contains_bytes(
    const uint8_t *haystack,
    size_t haystack_len,
    const uint8_t *needle,
    size_t needle_len)
{
    size_t index;
    if (needle_len == 0 || needle_len > haystack_len) {
        return 0;
    }
    for (index = 0; index <= haystack_len - needle_len; index++) {
        if (memcmp(haystack + index, needle, needle_len) == 0) {
            return 1;
        }
    }
    return 0;
}

static size_t write_u32(uint8_t *out, uint32_t value)
{
    out[0] = (uint8_t) (value >> 24);
    out[1] = (uint8_t) (value >> 16);
    out[2] = (uint8_t) (value >> 8);
    out[3] = (uint8_t) value;
    return 4;
}

static size_t write_text_bytes(
    uint8_t *out,
    uint8_t tag,
    const uint8_t *value,
    size_t len)
{
    out[0] = tag;
    write_u32(out + 1, (uint32_t) len);
    if (len != 0) {
        memcpy(out + 5, value, len);
    }
    return 5 + len;
}

static size_t write_text(uint8_t *out, uint8_t tag, const char *value)
{
    return write_text_bytes(
        out,
        tag,
        (const uint8_t *) value,
        strlen(value));
}

static size_t write_empty_map(uint8_t *out)
{
    out[0] = HTA_MAP;
    write_u32(out + 1, 0);
    return 5;
}

static size_t write_grant(uint8_t *out)
{
    size_t n = 0;
    out[n++] = HTA_MAP;
    n += write_u32(out + n, 3);
    n += write_text(out + n, HTA_KEYWORD, "write");
    out[n++] = HTA_FALSE;
    n += write_text(out + n, HTA_KEYWORD, "commands");
    out[n++] = HTA_SET;
    n += write_u32(out + n, 1);
    n += write_text(out + n, HTA_STRING, "status");
    n += write_text(out + n, HTA_KEYWORD, "protocol");
    n += write_text(out + n, HTA_STRING, "hoplite.console-grant/0-alpha");
    return n;
}

static size_t write_input(uint8_t *out)
{
    size_t n = 0;
    out[n++] = HTA_MAP;
    n += write_u32(out + n, 2);
    n += write_text(out + n, HTA_KEYWORD, "required");
    out[n++] = HTA_ORDERED_SET;
    n += write_u32(out + n, 0);
    n += write_text(out + n, HTA_KEYWORD, "values");
    out[n++] = HTA_SORTED_MAP;
    n += write_u32(out + n, 1);
    n += write_text(out + n, HTA_STRING, "answer");
    out[n++] = HTA_I64;
    memset(out + n, 0, 7);
    out[n + 7] = 42;
    n += 8;
    return n;
}

static size_t write_call(uint8_t *out, const char *extra, int duplicate_op)
{
    size_t n = 0;
    memcpy(out + n, "HTA0", 4);
    n += 4;
    out[n++] = HTA_MAP;
    n += write_u32(out + n, (uint32_t) (4 + (extra != NULL) + duplicate_op));
    n += write_text(out + n, HTA_KEYWORD, "op");
    n += write_text(out + n, HTA_STRING, "call");
    n += write_text(out + n, HTA_STRING, "grant");
    n += write_grant(out + n);
    n += write_text(out + n, HTA_KEYWORD, "command");
    n += write_text(out + n, HTA_STRING, "status");
    n += write_text(out + n, HTA_KEYWORD, "input");
    n += write_input(out + n);
    if (extra != NULL) {
        n += write_text(out + n, HTA_KEYWORD, extra);
        n += write_text(out + n, HTA_STRING, "tahto.node.console/dispatch");
    }
    if (duplicate_op) {
        n += write_text(out + n, HTA_KEYWORD, "op");
        n += write_text(out + n, HTA_STRING, "call");
    }
    return n;
}

static size_t write_sequence(uint8_t *out, uint8_t tag, uint32_t count)
{
    out[0] = tag;
    write_u32(out + 1, count);
    return 5;
}
