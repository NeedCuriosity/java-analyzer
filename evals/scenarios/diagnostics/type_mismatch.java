class Main {
    void run(String s, int n, Object o) {
        int a = /*@str_to_int*/"text";
        String b = /*@int_to_str*/42;
        boolean c = /*@int_to_bool*/n;
        String d = /*@obj_to_str*/o;
        int e = /*@double_to_int*/1.5;
        long ok1 = n;
        double ok2 = n;
        Object ok3 = s;
        Integer ok4 = n;
        int ok5 = 'c';
        byte ok6 = 10;
        CharSequence ok7 = s;
        float ok8 = 1;
        char ok9 = 65;
        Object ok10 = n;
        Number ok11 = 1;
        final int k = 10;
        byte ok12 = k;
    }

    int ret() { return /*@ret*/"x"; }
}
//? diag type-mismatch str_to_int
//? diag type-mismatch int_to_str
//? diag type-mismatch int_to_bool
//? diag type-mismatch obj_to_str
//? diag type-mismatch double_to_int
//? diag type-mismatch ret
//? diag exact
