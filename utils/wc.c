int main(void)
{
    int c;
    int lines = 0;
    int words = 0;
    int bytes = 0;
    int in_word = 0;
    int whitespace;

    while ((c = getchar()) != -1)
    {
        /* Prevent signed 32-bit counter overflow. */
        if (bytes == 2147483647)
        {
            return 1;
        }

        bytes++;

        if (c == '\n')
        {
            lines++;
        }

        /*
         * ASCII whitespace:
         * space, tab, newline, carriage return,
         * form feed (12), vertical tab (11).
         */
        whitespace = c == ' ' || c == '\t' || c == '\n'
                  || c == '\r' || c == 12 || c == 11;

        if (whitespace)
        {
            in_word = 0;
        }
        else
        {
            if (!in_word)
            {
                words++;
            }

            in_word = 1;
        }
    }

    if (printf("%d %d %d\n", lines, words, bytes) < 0)
    {
        return 1;
    }

    return 0;
}