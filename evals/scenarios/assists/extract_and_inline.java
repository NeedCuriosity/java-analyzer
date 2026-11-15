class Main {
    int compute(int n) {
        int doubled = /*@ev1*/n * 2/*@ev2*/;
        int /*@inline*/base = n + 1;
        int result = base * doubled;
        /*@em1*/System.out.println(result);
        System.out.println(n);/*@em2*/
        return result + /*@c1*/1000/*@c2*/;
    }

    private int twice(int v) { return v * 2; }

    int caller(int a) { return /*@call*/twice(a + 1) + 3; }
}
//? assist extract_variable ev1 ev2 ; has "int extracted = n * 2;" ; has "int doubled = extracted;"
//? assist inline_variable inline ; has "int result = (n + 1) * doubled;" ; lacks "int base"
//? assist extract_method em1 em2 ; has "extracted(n, result);" ; has "private void extracted(int n, int result)"
//? assist extract_constant c1 c2 ; has "static final int" ; has "= 1000;"
//? assist inline_method call ; has "(a + 1) * 2" ; has "+ 3;" ; lacks "twice"
