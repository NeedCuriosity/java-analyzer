class Main {
    static byte defaultByte = 0;
    static short defaultShort = 0;
    static char defaultChar = 'a';

    void setByte(byte b) {}
    void setShort(short s) {}
    void setChar(char c) {}

    void run(Object value, boolean flag, byte b, short s, int n, Byte boxedByte, Short boxedShort) {
        setByte(value == null ? defaultByte : (Byte) value);
        setShort(value == null ? defaultShort : (Short) value);
        setChar(value == null ? defaultChar : (Character) value);
        setShort(flag ? b : s);
        setShort(flag ? boxedByte : boxedShort);
        setByte(flag ? b : 0);
        setByte(flag ? -128 : boxedByte);
        setChar(flag ? defaultChar : 65);
        var /*@bb*/bb = flag ? b : boxedByte;
        var /*@bs*/bs = flag ? b : s;
        var /*@bc*/bc = flag ? b : defaultChar;
        var /*@big*/big = flag ? b : 200;
        var /*@bi*/bi = flag ? b : n;
        var /*@flags*/flags = flag ? flag : Boolean.TRUE;
        /*@int_arg*/setByte(flag ? b : n);
        /*@too_big*/setByte(flag ? b : 128);
        /*@char_short*/setShort(flag ? defaultChar : s);
    }
}
//? hover bb ~ "byte bb"
//? hover bs ~ "short bs"
//? hover bc ~ "int bc"
//? hover big ~ "int big"
//? hover bi ~ "int bi"
//? hover flags ~ "boolean flags"
//? diag argument-mismatch int_arg
//? diag argument-mismatch too_big
//? diag argument-mismatch char_short
//? diag exact
