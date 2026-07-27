using System.Buffers.Binary;
using System.Text.Json;

namespace Keyboop.Protocol;

public static class PipeFraming
{
    public static async ValueTask WriteAsync(
        Stream stream,
        ProtocolEnvelope message,
        CancellationToken cancellationToken = default)
    {
        var body = JsonSerializer.SerializeToUtf8Bytes(message, ProtocolJson.Options);
        if (body.Length > ProtocolConstants.MaximumFrameBytes)
        {
            throw new ProtocolException($"Message is too large: {body.Length} bytes.");
        }

        var header = new byte[sizeof(int)];
        BinaryPrimitives.WriteInt32LittleEndian(header, body.Length);
        await stream.WriteAsync(header, cancellationToken).ConfigureAwait(false);
        await stream.WriteAsync(body, cancellationToken).ConfigureAwait(false);
        await stream.FlushAsync(cancellationToken).ConfigureAwait(false);
    }

    public static async ValueTask<ProtocolEnvelope?> ReadAsync(
        Stream stream,
        CancellationToken cancellationToken = default)
    {
        var header = new byte[sizeof(int)];
        var firstRead = await ReadAtLeastAsync(
            stream,
            header,
            allowEndOfStream: true,
            cancellationToken).ConfigureAwait(false);
        if (firstRead == 0)
        {
            return null;
        }

        var length = BinaryPrimitives.ReadInt32LittleEndian(header);
        if (length is <= 0 or > ProtocolConstants.MaximumFrameBytes)
        {
            throw new ProtocolException($"Invalid frame length: {length}.");
        }

        var body = new byte[length];
        await ReadAtLeastAsync(
            stream,
            body,
            allowEndOfStream: false,
            cancellationToken).ConfigureAwait(false);
        var message = JsonSerializer.Deserialize<ProtocolEnvelope>(body, ProtocolJson.Options)
            ?? throw new ProtocolException("Message body is empty.");
        if (message.Version != ProtocolConstants.Version)
        {
            throw new ProtocolException(
                $"Protocol version {message.Version} is not supported; expected {ProtocolConstants.Version}.");
        }

        return message;
    }

    private static async ValueTask<int> ReadAtLeastAsync(
        Stream stream,
        Memory<byte> buffer,
        bool allowEndOfStream,
        CancellationToken cancellationToken)
    {
        var total = 0;
        while (total < buffer.Length)
        {
            var read = await stream.ReadAsync(buffer[total..], cancellationToken)
                .ConfigureAwait(false);
            if (read == 0)
            {
                if (total == 0 && allowEndOfStream)
                {
                    return 0;
                }

                throw new EndOfStreamException("Pipe closed in the middle of a message.");
            }

            total += read;
        }

        return total;
    }
}
