static int take(reader_t *reader, size_t amount, const uint8_t **output)
{
    if (reader == NULL || output == NULL || amount > reader->len
        || reader->cursor > reader->len - amount)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    *output = reader->data + reader->cursor;
    reader->cursor += amount;
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int read_u32(reader_t *reader, uint32_t *output)
{
    const uint8_t *bytes;
    if (output == NULL || take(reader, 4, &bytes) != 0) {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    *output = ((uint32_t) bytes[0] << 24)
            | ((uint32_t) bytes[1] << 16)
            | ((uint32_t) bytes[2] << 8)
            | (uint32_t) bytes[3];
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int read_data(reader_t *reader, hoplite_console_slice_t *output)
{
    const uint8_t *data;
    uint32_t length;
    if (output == NULL || read_u32(reader, &length) != 0
        || take(reader, (size_t) length, &data) != 0)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    output->data = data;
    output->len = (size_t) length;
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int skip_immutable_value(reader_t *reader, size_t depth);

static int skip_sequence(reader_t *reader, size_t depth, int require_member)
{
    uint32_t count;
    size_t index;
    size_t remaining;

    if (read_u32(reader, &count) != 0) {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    remaining = reader->len - reader->cursor;
    if ((require_member && count == 0u) || (size_t) count > remaining) {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    for (index = 0; index < (size_t) count; index++) {
        if (skip_immutable_value(reader, depth + 1u) != 0) {
            return HOPLITE_CONSOLE_TRANSPORT_ERROR;
        }
    }
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int skip_map(reader_t *reader, size_t depth)
{
    uint32_t count;
    size_t index;
    size_t remaining;

    if (read_u32(reader, &count) != 0) {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    remaining = reader->len - reader->cursor;
    if ((size_t) count > remaining / 2u) {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    for (index = 0; index < (size_t) count; index++) {
        if (skip_immutable_value(reader, depth + 1u) != 0
            || skip_immutable_value(reader, depth + 1u) != 0)
        {
            return HOPLITE_CONSOLE_TRANSPORT_ERROR;
        }
    }
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int skip_fixed_values(reader_t *reader, size_t depth, size_t count)
{
    size_t index;
    for (index = 0; index < count; index++) {
        if (skip_immutable_value(reader, depth + 1u) != 0) {
            return HOPLITE_CONSOLE_TRANSPORT_ERROR;
        }
    }
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int skip_immutable_value(reader_t *reader, size_t depth)
{
    const uint8_t *tag_data;
    const uint8_t *ignored;
    hoplite_console_slice_t data;

    if (depth > HOPLITE_CONSOLE_MAXIMUM_DEPTH
        || take(reader, 1, &tag_data) != 0)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }

    switch (tag_data[0]) {
    case HTA_NIL:
    case HTA_FALSE:
    case HTA_TRUE:
        return HOPLITE_CONSOLE_TRANSPORT_OK;
    case HTA_I64:
    case HTA_F64:
        return take(reader, 8, &ignored);
    case HTA_CHARACTER:
        return take(reader, 4, &ignored);
    case HTA_STRING:
    case HTA_BYTES:
    case HTA_KEYWORD:
    case HTA_SYMBOL:
    case HTA_BIG_INTEGER:
    case HTA_DECIMAL:
    case HTA_REGEX:
        return read_data(reader, &data);
    case HTA_LIST:
    case HTA_VECTOR:
    case HTA_SET:
    case HTA_TUPLE:
    case HTA_QUEUE:
    case HTA_ORDERED_SET:
    case HTA_SORTED_SET:
    case HTA_DEQUE:
        return skip_sequence(reader, depth, 0);
    case HTA_CONS:
        return skip_sequence(reader, depth, 1);
    case HTA_MAP:
    case HTA_ORDERED_MAP:
    case HTA_SORTED_MAP:
    case HTA_TRIE:
    case HTA_PRIORITY_MAP:
        return skip_map(reader, depth);
    case HTA_TAGGED:
    case HTA_POINTER:
        return skip_fixed_values(reader, depth, 2u);
    case HTA_EXCEPTION_INFO:
    case HTA_STRUCT:
        return skip_fixed_values(reader, depth, 3u);

    /* These tags carry live runtime, mutable, or opaque host identity. */
    case HTA_HANDLE:
    case HTA_NAMESPACE:
    case HTA_VAR:
    case HTA_ATOM:
    case HTA_ARRAY:
    case HTA_OBJECT:
    case HTA_VAR_REF:
    default:
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
}

static int read_value_slice(reader_t *reader, hoplite_console_slice_t *output)
{
    size_t start;
    if (reader == NULL || output == NULL) {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    start = reader->cursor;
    if (skip_immutable_value(reader, 0) != 0) {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    output->data = reader->data + start;
    output->len = reader->cursor - start;
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int text_equal(hoplite_console_slice_t value, const char *expected)
{
    size_t length = strlen(expected);
    return value.len == length
        && (length == 0 || memcmp(value.data, expected, length) == 0);
}

static int read_key(reader_t *reader, hoplite_console_slice_t *key)
{
    const uint8_t *tag;
    if (take(reader, 1, &tag) != 0
        || (tag[0] != HTA_STRING && tag[0] != HTA_KEYWORD)
        || read_data(reader, key) != 0)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int read_text_value(
    hoplite_console_slice_t encoded,
    hoplite_console_slice_t *text)
{
    reader_t reader;
    const uint8_t *tag;
    reader.data = encoded.data;
    reader.len = encoded.len;
    reader.cursor = 0;
    if (take(&reader, 1, &tag) != 0
        || (tag[0] != HTA_STRING && tag[0] != HTA_KEYWORD)
        || read_data(&reader, text) != 0
        || reader.cursor != reader.len)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}
