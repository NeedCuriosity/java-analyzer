import java.util.function.IntSupplier;

class Main {
    int local(int base) {
        int /*@outer_f*/f = 100;
        class Local {
            int /*@fdecl*/f;
            int /*@adecl*/a() { return /*@b_in_a*/b() + 1; }
            int /*@bdecl*/b() { return /*@f_in_b*/f + base + this./*@f_this*/f; }
        }
        Local l = new Local();
        l./*@f_write*/f = 3;
        int x = new Local()./*@f_new*/f + /*@outer_use*/f;
        IntSupplier s = () -> {
            class Inner {
                int g() { return new Local()./*@a_nested*/a(); }
            }
            return new Inner().g() + l./*@a_lambda*/a();
        };
        return l./*@a_direct*/a() + x + s.getAsInt();
    }

    int kinds() {
        record P(int /*@xdecl*/x) {
            int /*@dbldecl*/dbl() { return /*@x_in_dbl*/x * 2; }
        }
        enum E {
            A, B;
            int /*@kdecl*/k() { return ordinal(); }
        }
        interface I {
            int run();
            default int /*@twicedecl*/twice() { return /*@run_in_twice*/run() * 2; }
        }
        P p = new P(1);
        I i = () -> 1;
        return p./*@x*/x() + p./*@dbl*/dbl() + E.A./*@k*/k() + i./*@twice*/twice();
    }

    <T> T generic(T value) {
        class Box<U> {
            U /*@udecl*/u;
            T /*@tdecl*/t;
            Box(U u, T t) { this.u = u; this.t = t; }
            <V> V /*@convdecl*/conv(V v) { return v; }
        }
        Box<String> box = new Box<>("s", value);
        String s = box./*@u*/u;
        T t = box./*@t*/t;
        Integer n = box./*@conv*/conv(1);
        return t;
    }
}
//? def b_in_a -> bdecl
//? def f_in_b -> fdecl
//? def f_this -> fdecl
//? def f_write -> fdecl
//? def f_new -> fdecl
//? def outer_use -> outer_f
//? def a_nested -> adecl
//? def a_lambda -> adecl
//? def a_direct -> adecl
//? def x -> xdecl
//? def x_in_dbl -> xdecl
//? def dbl -> dbldecl
//? def k -> kdecl
//? def twice -> twicedecl
//? def u -> udecl
//? def t -> tdecl
//? def conv -> convdecl
//? hover f_new ~ "int f"
//? hover a_direct ~ "int a()"
//? hover u ~ "U u"
//? refs fdecl = f_in_b f_this f_write f_new
//? refs adecl = a_nested a_lambda a_direct
//? refs bdecl = b_in_a
//? refs outer_f = outer_use
//? diag none
