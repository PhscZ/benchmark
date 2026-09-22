int main(void)
{
    unsigned char prefix[4];
    int count = 0;
    int c;
    int i;

    while ((c = getchar()) != -1)
    {
        if (c >= 32 && c <= 126)
        {
            if (count < 4)
            {
                prefix[count] = c;
                count++;

                /* Only output a run once it reaches four bytes. */
                if (count == 4)
                {
                    for (i = 0; i < 4; i++)
                    {
                        if (putchar(prefix[i]) == -1)
                        {
                            return 1;
                        }
                    }
                }
            }
            else
            {
                /* Stream the rest without buffering the entire run. */
                if (putchar(c) == -1)
                {
                    return 1;
                }
            }
        }
        else
        {
            if (count == 4)
            {
                if (putchar('\n') == -1)
                {
                    return 1;
                }
            }

            count = 0;
        }
    }

    /* Finish a qualifying run that ends at EOF. */
    if (count == 4)
    {
        if (putchar('\n') == -1)
        {
            return 1;
        }
    }

    return 0;
}