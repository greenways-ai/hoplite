size_t hoplite_console_call_encoded_size(const hoplite_console_call_t *call)
{
    hoplite_console_slice_t command_text;
    size_t total = sizeof(hta_magic) + 1u + 4u;
    if (call == NULL || call->grant.data == NULL || call->grant.len == 0
        || call->command.data == NULL || call->command.len == 0
        || call->input.data == NULL || call->input.len == 0
        || hoplite_console_value_validate(call->grant.data, call->grant.len) != 0
        || read_text_value(call->command, &command_text) != 0
        || command_text.len == 0
        || hoplite_console_value_validate(call->input.data, call->input.len) != 0
        || checked_add(&total, text_encoded_size(strlen("grant"))) != 0
        || checked_add(&total, call->grant.len) != 0
        || checked_add(&total, text_encoded_size(strlen("input"))) != 0
        || checked_add(&total, call->input.len) != 0
        || checked_add(&total, text_encoded_size(strlen("command"))) != 0
        || checked_add(&total, call->command.len) != 0)
    {
        return 0;
    }
    return total;
}

int hoplite_console_call_encode(
    const hoplite_console_call_t *call,
    uint8_t *output,
    size_t capacity,
    size_t *written)
{
    writer_t writer;
    size_t required = hoplite_console_call_encoded_size(call);
    if (written != NULL) {
        *written = 0;
    }
    if (required == 0 || output == NULL || written == NULL
        || required > capacity)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    writer.data = output;
    writer.len = capacity;
    writer.cursor = 0;
    if (write_bytes(&writer, hta_magic, sizeof(hta_magic)) != 0
        || write_byte(&writer, HTA_MAP) != 0
        || write_u32(&writer, 3u) != 0
        || write_keyword(&writer, "grant") != 0
        || write_bytes(&writer, call->grant.data, call->grant.len) != 0
        || write_keyword(&writer, "input") != 0
        || write_bytes(&writer, call->input.data, call->input.len) != 0
        || write_keyword(&writer, "command") != 0
        || write_bytes(&writer, call->command.data, call->command.len) != 0
        || writer.cursor != required)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    *written = writer.cursor;
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

size_t hoplite_console_success_encoded_size(size_t value_len)
{
    size_t total = sizeof(hta_magic) + 1u + 4u;
    if (value_len == 0
        || checked_add(&total, text_encoded_size(strlen("ok"))) != 0
        || checked_add(&total, 1u) != 0
        || checked_add(&total, text_encoded_size(strlen("value"))) != 0
        || checked_add(&total, value_len) != 0)
    {
        return 0;
    }
    return total;
}

int hoplite_console_success_encode(
    const uint8_t *value,
    size_t value_len,
    uint8_t *output,
    size_t capacity,
    size_t *written)
{
    writer_t writer;
    size_t required = hoplite_console_success_encoded_size(value_len);
    if (written != NULL) {
        *written = 0;
    }
    if (required == 0 || value == NULL || output == NULL || written == NULL
        || required > capacity
        || hoplite_console_value_validate(value, value_len) != 0)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    writer.data = output;
    writer.len = capacity;
    writer.cursor = 0;
    if (write_bytes(&writer, hta_magic, sizeof(hta_magic)) != 0
        || write_byte(&writer, HTA_MAP) != 0
        || write_u32(&writer, 2u) != 0
        || write_keyword(&writer, "ok") != 0
        || write_byte(&writer, HTA_TRUE) != 0
        || write_keyword(&writer, "value") != 0
        || write_bytes(&writer, value, value_len) != 0
        || writer.cursor != required)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    *written = writer.cursor;
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

size_t hoplite_console_failure_encoded_size(
    size_t code_len,
    size_t message_len)
{
    size_t total = sizeof(hta_magic) + 1u + 4u;
    if (code_len > UINT32_MAX || message_len > UINT32_MAX
        || checked_add(&total, text_encoded_size(strlen("ok"))) != 0
        || checked_add(&total, 1u) != 0
        || checked_add(&total, text_encoded_size(strlen("error"))) != 0
        || checked_add(&total, 1u + 4u) != 0
        || checked_add(&total, text_encoded_size(strlen("code"))) != 0
        || checked_add(&total, text_encoded_size(code_len)) != 0
        || checked_add(&total, text_encoded_size(strlen("message"))) != 0
        || checked_add(&total, text_encoded_size(message_len)) != 0)
    {
        return 0;
    }
    return total;
}

int hoplite_console_failure_encode(
    const uint8_t *code,
    size_t code_len,
    const uint8_t *message,
    size_t message_len,
    uint8_t *output,
    size_t capacity,
    size_t *written)
{
    writer_t writer;
    size_t required = hoplite_console_failure_encoded_size(code_len, message_len);
    if (written != NULL) {
        *written = 0;
    }
    if (required == 0 || output == NULL || written == NULL
        || (code_len != 0 && code == NULL)
        || (message_len != 0 && message == NULL)
        || required > capacity)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    writer.data = output;
    writer.len = capacity;
    writer.cursor = 0;
    if (write_bytes(&writer, hta_magic, sizeof(hta_magic)) != 0
        || write_byte(&writer, HTA_MAP) != 0
        || write_u32(&writer, 2u) != 0
        || write_keyword(&writer, "ok") != 0
        || write_byte(&writer, HTA_FALSE) != 0
        || write_keyword(&writer, "error") != 0
        || write_byte(&writer, HTA_MAP) != 0
        || write_u32(&writer, 2u) != 0
        || write_keyword(&writer, "code") != 0
        || write_text(&writer, HTA_STRING, code, code_len) != 0
        || write_keyword(&writer, "message") != 0
        || write_text(&writer, HTA_STRING, message, message_len) != 0
        || writer.cursor != required)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    *written = writer.cursor;
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}

int hoplite_console_frame_encode(
    const uint8_t *payload,
    size_t payload_len,
    size_t maximum_payload_bytes,
    uint8_t *output,
    size_t capacity,
    size_t *written)
{
    writer_t writer;
    if (written != NULL) {
        *written = 0;
    }
    if (payload == NULL || payload_len == 0 || output == NULL || written == NULL
        || payload_len > maximum_payload_bytes || payload_len > UINT32_MAX
        || capacity < HOPLITE_CONSOLE_FRAME_PREFIX_BYTES
        || payload_len > capacity - HOPLITE_CONSOLE_FRAME_PREFIX_BYTES)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    writer.data = output;
    writer.len = capacity;
    writer.cursor = 0;
    if (write_u32(&writer, (uint32_t) payload_len) != 0
        || write_bytes(&writer, payload, payload_len) != 0)
    {
        return HOPLITE_CONSOLE_TRANSPORT_ERROR;
    }
    *written = writer.cursor;
    return HOPLITE_CONSOLE_TRANSPORT_OK;
}
