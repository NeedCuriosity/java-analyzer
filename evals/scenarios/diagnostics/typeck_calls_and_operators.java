import java.util.List;

class Main {
    void take(int n) {}
    void two(String a, int b) {}
    void over(int a) {}
    void over(String a) {}
    void many(String first, int... rest) {}
    Main(String name) {}
    Main() {}

    void run(String s, int n, boolean flag, List<String> names, Object o) {
        /*@arity*/take();
        /*@arity2*/take(1, 2);
        /*@wrong_ty*/take("x");
        /*@wrong_ty2*/two(n, s);
        /*@overload*/over(flag);
        new /*@ctor*/Main(1, 2);
        new /*@ctor_ty*/Main(flag);
        names./*@generic_recv*/add(n);
        s./*@lib*/charAt("0");

        if (/*@cond*/n) {}
        while (/*@cond2*/s) {}
        int bad1 = /*@minus*/s - 1;
        boolean bad2 = /*@and*/n && flag;
        boolean bad3 = /*@not*/!n;
        boolean bad4 = /*@cmp*/flag < flag;
        boolean bad5 = /*@eq*/n == flag;
        int bad6 = /*@neg*/-flag;

        take(n);
        take('c');
        two(s, n);
        over(n);
        over(s);
        many(s);
        many(s, 1, 2, 3);
        new Main(s);
        names.add(s);
        long ok1 = n + 1L;
        String ok2 = s + n + flag + o;
        boolean ok3 = n < 2.5 && flag || !flag;
        boolean ok4 = o == s;
        boolean ok5 = flag == true;
        Integer boxed = n;
        boolean ok6 = boxed == n;
        int ok7 = n & 3 | n ^ 1;
        boolean ok8 = flag & flag | flag ^ flag;
        int ok9 = 'a' + 1;
        if (flag) {}
        Boolean wrapped = flag;
        if (wrapped) {}
        short small = 1;
        take(small);
        Object ok10 = n;
        double ok11 = -n * 2.0 % 3;
    }
}
//? diag argument-mismatch arity
//? diag argument-mismatch arity2
//? diag argument-mismatch wrong_ty
//? diag argument-mismatch wrong_ty2
//? diag argument-mismatch overload
//? diag argument-mismatch ctor
//? diag argument-mismatch ctor_ty
//? diag argument-mismatch generic_recv
//? diag argument-mismatch lib
//? diag type-mismatch cond
//? diag type-mismatch cond2
//? diag bad-operand minus
//? diag bad-operand and
//? diag bad-operand not
//? diag bad-operand cmp
//? diag bad-operand eq
//? diag bad-operand neg
//? diag exact
