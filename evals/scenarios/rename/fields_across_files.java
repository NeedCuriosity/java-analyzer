//- /app/User.java
package app;

public class User {
    public String /*@field*/name;
    private int age;

    public User(String name) { this.name = name; }

    public String describe() { return name + age; }
}
//- /app/Service.java
package app;

class Service {
    String greet(User user, String name) {
        return "hi " + user.name + name;
    }
}
//? rename field label ; has "public String label;" ; has "this.label = name;" ; has "return label + age;" ; has "user.label + name" ; lacks "user.name"
//? rename field age ; fails
