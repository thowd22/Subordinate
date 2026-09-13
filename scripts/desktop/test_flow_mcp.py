"""Desktop MCP project-response parsing, without starting a desktop."""
import unittest

from flow_mcp import _name_of, _project
from flow_clicks import _project as clicks_project


class ProjectResponseTests(unittest.TestCase):
    def test_project_get_unwraps_result_and_project_file_envelopes(self):
        project = {"name": "Main", "sequences": [{"id": "sequence_1"}]}
        response = {"revision": 3, "project": {"schema_version": 1, "project": project}}
        self.assertEqual(_project(response)["sequences"][0]["id"], "sequence_1")
        self.assertEqual(_name_of(response), "Main")
        self.assertEqual(clicks_project(response)["sequences"][0]["id"], "sequence_1")

    def test_empty_project_and_direct_shape_remain_supported(self):
        project = {"name": "Untitled", "sequences": []}
        for response in (project, {"project": project},
                         {"project": {"schema_version": 1, "project": project}}):
            self.assertEqual(_project(response), project)

    def test_malformed_response_cannot_pass_as_an_empty_project(self):
        for response in ({"revision": 0}, {"project": {"schema_version": 1}},
                         {"project": {"sequences": None}}, None):
            with self.assertRaisesRegex(AssertionError, "project.get did not contain a project"):
                _project(response)


if __name__ == "__main__":
    unittest.main()
