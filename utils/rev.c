unsigned char buffer[65536];

int write_reversed(int length)
{
    while (length > 0)
    {
        length--;

        if (putchar(buffer[length]) == -1)
        {
            return 1;
        }
    }

    return 0;
}

int main(void)
{
    int c;
    int length = 0;

    while ((c = getchar()) != -1)
    {
        if (c == '\n')
        {
            if (write_reversed(length) != 0)
            {
                return 1;
            }

            if (putchar('\n') == -1)
            {
                return 1;
            }

            length = 0;
        }
        else
        {
            /* Reject oversized lines without overflowing the buffer. */
            if (length == 65536)
            {
                return 1;
            }

            buffer[length] = c;
            length++;
        }
    }

    /* Preserve a missing final newline. */
    if (write_reversed(length) != 0)
    {
        return 1;
    }

    return 0;
}