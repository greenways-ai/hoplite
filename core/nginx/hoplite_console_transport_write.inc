static int write_bytes(writer_t *writer, const void *data, size_t len)
{
    if (writer == NULL || (len != 0 && data == NULL)
        || len > writer->len || writer->cursor > writer->len - len)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    if (len != 0) {
        memcpy(writer->data + writer->cursor, data, len);
    }
    writer->cursor += len;
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int write_byte(writer_t *writer, uint8_t value)
{
    return write_bytes(writer, &value, 1);
}

static int write_u32(writer_t *writer, uint32_t value)
{
    uint8_t bytes[4];
    bytes[0] = (uint8_t) (value >> 24);
    bytes[1] = (uint8_t) (value >> 16);
    bytes[2] = (uint8_t) (value >> 8);
    bytes[3] = (uint8_t) value;
    return write_bytes(writer, bytes, sizeof(bytes));
}

static int write_text(
    writer_t *writer,
    uint8_t tag,
    const uint8_t *data,
    size_t len)
{
    if (len > UINT32_MAX || (len != 0 && data == NULL)
        || write_byte(writer, tag) != 0
        || write_u32(writer, (uint32_t) len) != 0
        || write_bytes(writer, data, len) != 0)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static size_t text_encoded_size(size_t len)
{
    return len <= UINT32_MAX && len <= SIZE_MAX - 5u ? 5u + len : SIZE_MAX;
}

static int checked_add(size_t *total, size_t amount)
{
    if (total == NULL || amount == SIZE_MAX || *total > SIZE_MAX - amount) {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    *total += amount;
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

static int write_keyword(writer_t *writer, const char *value)
{
    return write_text(
        writer,
        HTA_KEYWORD,
        (const uint8_t *) value,
        strlen(value));
}
