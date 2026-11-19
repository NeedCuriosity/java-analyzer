class InlineOverload {
    static String render(Object n) { return "Object"; }
    static String render(String n) { return "String"; }

    private static String wrap(Object n) { return render(n); }

    private static long square(long n) { return n * n; }

    public static void main(String[] args) {
        System.out.println(/*@wrap*/wrap("x"));
        int big = 100000;
        System.out.println(/*@square*/square(big));
    }
}
//? assist inline_method wrap ; has "System.out.println(render((Object) \"x\"));" ; lacks "wrap"
//? assist inline_method square ; has "System.out.println(((long) big) * ((long) big));" ; lacks "square"
