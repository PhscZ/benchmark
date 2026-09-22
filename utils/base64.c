int main(void)
{
    char *alphabet =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    unsigned char input[3];
    char output[4];

    int c;
    int count;
    int value;
    int i;

    while (1)
    {
        input[0] = 0;
        input[1] = 0;
        input[2] = 0;
        count = 0;

        while (count < 3)
        {
            c = getchar();

            if (c == -1)
            {
                break;
            }

            input[count] = c;
            count++;
        }

        if (count == 0)
        {
            break;
        }

        /* Pack up to three bytes into a nonnegative 24-bit integer. */
        value = (input[0] << 16) | (input[1] << 8) | input[2];

        output[0] = alphabet[(value >> 18) & 63];
        output[1] = alphabet[(value >> 12) & 63];
        output[2] = '=';
        output[3] = '=';

        if (count >= 2)
        {
            output[2] = alphabet[(value >> 6) & 63];
        }

        if (count == 3)
        {
            output[3] = alphabet[value & 63];
        }

        for (i = 0; i < 4; i++)
        {
            if (putchar(output[i]) == -1)
            {
                return 1;
            }
        }

        if (count < 3)
        {
            break;
        }
    }

    return 0;
}