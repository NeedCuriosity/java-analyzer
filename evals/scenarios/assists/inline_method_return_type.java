class InlineReturn {
    static String render(Object n) { return "Object"; }
    static String render(String n) { return "String"; }

    private static Object get() { return "x"; }

    private static int id(int x) { return x; }

    private static long big() { return 100000; }

    public static void main(String[] args) {
        System.out.println(render(/*@get*/get()));
        System.out.println(/*@id*/id('a'));
        System.out.println(/*@big*/big() * 100000);
    }
}
//? assist inline_method get ; has "System.out.println(render((Object) \"x\"));" ; lacks "get()"
//? assist inline_method id ; has "System.out.println((int) 'a');" ; lacks "id("
//? assist inline_method big ; has "System.out.println(((long) 100000) * 100000);" ; lacks "big()"
