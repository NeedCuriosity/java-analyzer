import java.util.List;

class Main {
    private int counts/*@c1*/[];
    private int a, b[];
    private static final String SENTINEL = new String();

    String names()/*@c2*/[] {
        return new String[0];
    }

    void params(String args/*@c3*/[], String... rest) {
        int grid/*@c4*/[][] = new int[2][2];
        int[] fine = new int[1];
        System.out.println(args.length + rest.length + grid.length + fine.length + counts.length + a + b.length);
    }

    String strings(String s, Object o, List<String> list, int n) {
        String t = s/*@t1*/.toString();
        String u = "o=" + o/*@t2*/.toString();
        String v = o/*@t3*/.toString() + s;
        String w = o.toString() + list.toString();
        String x = n + o.toString();
        String y = super.toString();
        String z = new /*@s1*/String("literal");
        String e = new /*@s2*/String();
        int len = new /*@s3*/String(s + t).length();
        String chars = new String(new char[] {'a'});
        return SENTINEL + t + u + v + w + x + y + z + e + len + chars;
    }
}
//? diag c-style-array c1
//? diag c-style-array c2
//? diag c-style-array c3
//? diag c-style-array c4
//? diag only c-style-array
//? diag redundant-to-string t1
//? diag redundant-to-string t2
//? diag redundant-to-string t3
//? diag only redundant-to-string
//? diag redundant-string-constructor s1
//? diag redundant-string-constructor s2
//? diag redundant-string-constructor s3
//? diag only redundant-string-constructor
//? assist fix_redundancy c1 ; has "private int[] counts;"
//? assist fix_redundancy c2 ; has "String[] names() {"
//? assist fix_redundancy c3 ; has "void params(String[] args, String... rest)"
//? assist fix_redundancy c4 ; has "int[][] grid = new int[2][2];"
//? assist fix_redundancy t1 ; has "String t = s;"
//? assist fix_redundancy t2 ; has "String u = \"o=\" + o;"
//? assist fix_redundancy s1 ; has "String z = \"literal\";"
//? assist fix_redundancy s2 ; has "String e = \"\";"
//? assist fix_redundancy s3 ; has "int len = (s + t).length();"
