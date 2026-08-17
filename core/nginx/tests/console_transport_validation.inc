static void test_fragmented_frame_reader(void)
{
    uint8_t payload[768];
    uint8_t frame[772];
    uint8_t storage[768];
    size_t payload_len = write_call(payload, NULL, 0);
    size_t frame_len;
    size_t consumed;
    hoplite_console_frame_reader_t reader;
    hoplite_console_slice_t result;

    assert(hoplite_console_frame_encode(
        payload, payload_len, sizeof(storage), frame, sizeof(frame), &frame_len) == 0);
    hoplite_console_frame_reader_init(&reader, storage, sizeof(storage));
    assert(hoplite_console_frame_reader_feed(&reader, frame, 2, &consumed) == 1);
    assert(consumed == 2);
    assert(hoplite_console_frame_reader_feed(&reader, frame + 2, 7, &consumed) == 1);
    assert(consumed == 7);
    assert(hoplite_console_frame_reader_feed(
        &reader, frame + 9, frame_len - 9, &consumed) == 0);
    assert(consumed == frame_len - 9);
    result = hoplite_console_frame_reader_payload(&reader);
    assert(result.len == payload_len);
    assert(memcmp(result.data, payload, payload_len) == 0);
    assert(hoplite_console_frame_reader_feed(&reader, frame, 1, &consumed) == -1);
}

static void test_frame_bounds(void)
{
    uint8_t storage[8];
    uint8_t prefix[4];
    size_t consumed;
    hoplite_console_frame_reader_t reader;

    write_u32(prefix, 9);
    hoplite_console_frame_reader_init(&reader, storage, sizeof(storage));
    assert(hoplite_console_frame_reader_feed(&reader, prefix, sizeof(prefix), &consumed) == -1);
    assert(consumed == sizeof(prefix));

    write_u32(prefix, 0);
    hoplite_console_frame_reader_init(&reader, storage, sizeof(storage));
    assert(hoplite_console_frame_reader_feed(&reader, prefix, sizeof(prefix), &consumed) == -1);
}

static void test_exact_call_is_closed_before_application_entry(void)
{
    uint8_t payload[768];
    uint8_t closed[768];
    size_t payload_len = write_call(payload, NULL, 0);
    size_t closed_len;
    hoplite_console_call_t call;
    hoplite_console_call_t reparsed;

    assert(hoplite_console_call_parse(payload, payload_len, &call) == 0);
    assert(hoplite_console_call_encode(
        &call, closed, sizeof(closed), &closed_len) == 0);
    assert(closed_len < payload_len);
    assert(memcmp(closed, "HTA0", 4) == 0);
    {
        const uint8_t op_key[] = {HTA_KEYWORD, 0, 0, 0, 2, 'o', 'p'};
        assert(!contains_bytes(closed, closed_len, op_key, sizeof(op_key)));
    }
    assert(!contains_bytes(
        closed, closed_len, (const uint8_t *) "handler", strlen("handler")));

    /* A closed envelope is deliberately not a valid broker request. */
    assert(hoplite_console_call_parse(closed, closed_len, &reparsed) == -1);
    assert(contains_bytes(
        closed, closed_len, (const uint8_t *) "grant", strlen("grant")));
    assert(contains_bytes(
        closed, closed_len, (const uint8_t *) "command", strlen("command")));
    assert(contains_bytes(
        closed, closed_len, (const uint8_t *) "input", strlen("input")));
}

static void test_routing_and_duplicate_fields_fail_closed(void)
{
    uint8_t payload[1024];
    size_t len;
    hoplite_console_call_t call;

    len = write_call(payload, "handler", 0);
    assert(hoplite_console_call_parse(payload, len, &call) == -1);

    len = write_call(payload, NULL, 1);
    assert(hoplite_console_call_parse(payload, len, &call) == -1);

    len = write_call(payload, NULL, 0);
    payload[len++] = HTA_NIL;
    assert(hoplite_console_call_parse(payload, len, &call) == -1);

    len = write_call(payload, NULL, 0);
    memcpy(payload, "HTA1", 4);
    assert(hoplite_console_call_parse(payload, len, &call) == -1);
}

static void test_full_immutable_hta0_grammar(void)
{
    uint8_t value[512];
    size_t n;

    n = write_text(value, HTA_SYMBOL, "tahto/status");
    assert(hoplite_console_value_validate(value, n) == 0);

    n = write_sequence(value, HTA_TUPLE, 3);
    value[n++] = HTA_TRUE;
    value[n++] = HTA_F64;
    memset(value + n, 0, 8);
    n += 8;
    value[n++] = HTA_CHARACTER;
    memset(value + n, 0, 3);
    value[n + 3] = 'A';
    n += 4;
    assert(hoplite_console_value_validate(value, n) == 0);

    n = write_sequence(value, HTA_CONS, 1);
    n += write_text(value + n, HTA_BIG_INTEGER, "9223372036854775808");
    assert(hoplite_console_value_validate(value, n) == 0);

    n = write_sequence(value, HTA_QUEUE, 2);
    n += write_text(value + n, HTA_DECIMAL, "1.25");
    n += write_text(value + n, HTA_REGEX, "^ready$");
    assert(hoplite_console_value_validate(value, n) == 0);

    n = write_sequence(value, HTA_DEQUE, 1);
    n += write_text_bytes(value + n, HTA_BYTES, (const uint8_t *) "\0\1", 2);
    assert(hoplite_console_value_validate(value, n) == 0);

    n = 0;
    value[n++] = HTA_PRIORITY_MAP;
    n += write_u32(value + n, 1);
    n += write_text(value + n, HTA_KEYWORD, "priority");
    value[n++] = HTA_I64;
    memset(value + n, 0, 8);
    n += 8;
    assert(hoplite_console_value_validate(value, n) == 0);

    n = 0;
    value[n++] = HTA_TAGGED;
    n += write_text(value + n, HTA_SYMBOL, "demo/tag");
    n += write_empty_map(value + n);
    assert(hoplite_console_value_validate(value, n) == 0);

    n = 0;
    value[n++] = HTA_EXCEPTION_INFO;
    n += write_text(value + n, HTA_STRING, "failed");
    n += write_empty_map(value + n);
    value[n++] = HTA_NIL;
    assert(hoplite_console_value_validate(value, n) == 0);

    n = 0;
    value[n++] = HTA_STRUCT;
    n += write_text(value + n, HTA_STRING, "demo/State");
    n += write_sequence(value + n, HTA_VECTOR, 0);
    n += write_sequence(value + n, HTA_VECTOR, 0);
    assert(hoplite_console_value_validate(value, n) == 0);

    n = 0;
    value[n++] = HTA_POINTER;
    n += write_text(value + n, HTA_KEYWORD, "console");
    n += write_empty_map(value + n);
    assert(hoplite_console_value_validate(value, n) == 0);
}

static void test_live_and_mutable_hta0_tags_are_rejected(void)
{
    const uint8_t rejected[] = {
        HTA_HANDLE,
        HTA_NAMESPACE,
        HTA_VAR,
        HTA_ATOM,
        HTA_ARRAY,
        HTA_OBJECT,
        HTA_VAR_REF
    };
    size_t index;
    uint8_t value[16];

    for (index = 0; index < sizeof(rejected); index++) {
        value[0] = rejected[index];
        memset(value + 1, 0, sizeof(value) - 1);
        assert(hoplite_console_value_validate(value, sizeof(value)) == -1);
    }
}

static void test_malformed_and_overdeep_values_fail_closed(void)
{
    uint8_t payload[4096];
    size_t len = write_call(payload, NULL, 0);
    hoplite_console_call_t call;
    size_t i;

    assert(hoplite_console_call_parse(payload, len - 1, &call) == -1);

    memcpy(payload, "HTA0", 4);
    payload[4] = HTA_MAP;
    write_u32(payload + 5, 4);
    len = 9;
    len += write_text(payload + len, HTA_KEYWORD, "op");
    len += write_text(payload + len, HTA_STRING, "call");
    len += write_text(payload + len, HTA_KEYWORD, "grant");
    for (i = 0; i < HOPLITE_CONSOLE_MAXIMUM_DEPTH + 2; i++) {
        len += write_sequence(payload + len, HTA_VECTOR, 1);
    }
    payload[len++] = HTA_NIL;
    len += write_text(payload + len, HTA_KEYWORD, "command");
    len += write_text(payload + len, HTA_STRING, "status");
    len += write_text(payload + len, HTA_KEYWORD, "input");
    len += write_empty_map(payload + len);
    assert(hoplite_console_call_parse(payload, len, &call) == -1);

    payload[0] = HTA_CONS;
    write_u32(payload + 1, 0);
    assert(hoplite_console_value_validate(payload, 5) == -1);
}
