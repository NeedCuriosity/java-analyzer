import java.util.List;

class Main {
    private List<String> names;

    void copy(int[] src, int[] dst, String label) {
        System.out.println(label);
        /*@static_member*/.arraycopy(src, 0, dst, 0, 1);
        /*@instance_member*/.isEmpty();
    }
}
//? complete static_member first System
//? complete instance_member order names src
//? complete instance_member order label dst
