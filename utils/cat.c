int main(void)
{
    int c;

    while ((c = getchar()) != -1)
    {
        if (putchar(c) == -1)
        {
            return 1;
        }
    }

    return 0;
}