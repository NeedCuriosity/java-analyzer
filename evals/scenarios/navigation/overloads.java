class Printer {
    void /*@p_int*/print(int value) {}
    void /*@p_str*/print(String value) {}
    void /*@p_two*/print(String label, int value) {}
    void /*@p_obj*/print(Object value) {}
    void /*@p_long*/print(long value) {}

    void run(Printer other) {
        /*@c_int*/print(1);
        /*@c_str*/print("a");
        /*@c_two*/print("a", 2);
        other./*@c_obj*/print(new StringBuilder());
        /*@c_long*/print(1L);
        /*@c_char*/print('c');
        /*@c_boxed*/print(Integer.valueOf(3));
    }
}
//? def c_int -> p_int
//? def c_str -> p_str
//? def c_two -> p_two
//? def c_obj -> p_obj
//? def c_long -> p_long
//? def c_char -> p_int
//? def c_boxed -> p_obj
//? diag none
