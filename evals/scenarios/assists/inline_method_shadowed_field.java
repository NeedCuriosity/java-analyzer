class InlineField {
    static String name = "field";
    int count;

    private static String fetch() { return name; }

    private int current() { return count + 1; }

    public static void main(String[] args) {
        String name = "local";
        System.out.println(/*@fetch*/fetch() + name);
    }

    int scaled(int count) {
        return /*@current*/current() * count;
    }
}
//? assist inline_method fetch ; has "System.out.println(InlineField.name + name);" ; lacks "fetch"
//? assist inline_method current ; has "return (this.count + 1) * count;" ; lacks "current"
